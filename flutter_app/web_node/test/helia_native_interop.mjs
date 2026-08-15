import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'
import { createInterface } from 'node:readline'

import { noise } from '@chainsafe/libp2p-noise'
import { yamux } from '@chainsafe/libp2p-yamux'
import { withBitswap } from '@helia/bitswap'
import { withLibp2p } from '@helia/libp2p'
import { unixfs } from '@helia/unixfs'
import { identify } from '@libp2p/identify'
import { webSockets } from '@libp2p/websockets'
import { multiaddr } from '@multiformats/multiaddr'
import { createHeliaLight } from 'helia'
import { createLibp2p } from 'libp2p'
import { CID } from 'multiformats/cid'

const here = dirname(fileURLToPath(import.meta.url))
const backend = resolve(here, '../../../backend')
const fixture = spawn(
  'cargo',
  ['run', '--quiet', '-p', 'app-core', '--example', 'p2p_helia_fixture'],
  { cwd: backend, stdio: ['pipe', 'pipe', 'inherit'] }
)

const fixtureExit = new Promise((resolveExit, rejectExit) => {
  fixture.once('error', rejectExit)
  fixture.once('exit', (code, signal) => resolveExit({ code, signal }))
})

async function fixtureMetadata () {
  const lines = createInterface({ input: fixture.stdout })
  const timer = setTimeout(() => fixture.kill('SIGTERM'), 300_000)
  try {
    for await (const line of lines) {
      if (line.trim().startsWith('{')) return JSON.parse(line)
    }
    const result = await fixtureExit
    throw new Error(`native fixture exited before readiness: ${JSON.stringify(result)}`)
  } finally {
    clearTimeout(timer)
    lines.close()
  }
}

let node
try {
  const metadata = await fixtureMetadata()
  const libp2p = await createLibp2p({
    addresses: { listen: [] },
    transports: [webSockets()],
    connectionEncrypters: [noise()],
    streamMuxers: [yamux()],
    connectionGater: { denyDialMultiaddr: () => false },
    services: { identify: identify() }
  })
  node = withBitswap(withLibp2p(createHeliaLight(), libp2p))
  await node.start()
  await node.libp2p.dial(multiaddr(metadata.address), {
    signal: AbortSignal.timeout(30_000)
  })

  const chunks = []
  let byteLength = 0
  for await (const chunk of unixfs(node).cat(CID.parse(metadata.cid.replace(/^\/ipfs\//, '')), {
    signal: AbortSignal.timeout(30_000)
  })) {
    chunks.push(chunk)
    byteLength += chunk.byteLength
  }
  const bytes = Buffer.concat(chunks.map(chunk => Buffer.from(chunk)))
  const digest = createHash('sha256').update(bytes).digest('hex')
  if (byteLength !== metadata.byte_length) {
    throw new Error(`UnixFS length mismatch: expected ${metadata.byte_length}, got ${byteLength}`)
  }
  if (digest !== metadata.sha256) {
    throw new Error(`UnixFS digest mismatch: expected ${metadata.sha256}, got ${digest}`)
  }
  process.stdout.write(`Helia fetched and verified ${byteLength} bytes from ${metadata.cid}\n`)
} finally {
  await node?.stop()
  fixture.stdin.end('\n')
  const result = await fixtureExit
  if (result.code !== 0) {
    throw new Error(`native fixture failed: ${JSON.stringify(result)}`)
  }
}
