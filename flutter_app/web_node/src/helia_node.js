import { withBitswap } from '@helia/bitswap'
import { libp2pDefaults, withLibp2p } from '@helia/libp2p'
import { unixfs } from '@helia/unixfs'
import { webTransport } from '@libp2p/webtransport'
import { multiaddr } from '@multiformats/multiaddr'
import { IDBBlockstore } from 'blockstore-idb'
import { IDBDatastore } from 'datastore-idb'
import { createHeliaLight } from 'helia'
import { CID } from 'multiformats/cid'

const MAX_CAT_BYTES = 512 * 1024 * 1024
let node
let fs
let lifecycle = 'stopped'
let lastError = null
let bytesUp = 0
let bytesDown = 0

function createNode () {
  const defaults = libp2pDefaults({
    name: 'jimmusic-web',
    version: '2.0.0'
  })
  defaults.transports = [
    ...(defaults.transports ?? []),
    webTransport()
  ]
  defaults.connectionGater = {
    denyDialMultiaddr: () => false
  }

  // The browser node has no HTTP gateway or delegated HTTP router. Content
  // reaches the blockstore through Bitswap after libp2p discovery/dialing, so
  // disabling a public gateway cannot silently remove the only fetch path.
  delete defaults.services.delegatedContentRouting
  delete defaults.services.delegatedPeerRouting

  const blockstore = new IDBBlockstore('jimmusic-v2/blocks')
  const datastore = new IDBDatastore('jimmusic-v2/data')
  const helia = createHeliaLight({ blockstore, datastore })
  return withBitswap(withLibp2p(helia, defaults))
}

async function start () {
  if (node?.status === 'started') return status()
  lifecycle = 'starting'
  lastError = null
  try {
    node ??= createNode()
    fs ??= unixfs(node)
    await node.start()
    lifecycle = 'running'
    return status()
  } catch (error) {
    lifecycle = 'failed'
    lastError = String(error)
    throw error
  }
}

async function stop () {
  if (node != null && node.status !== 'stopped') await node.stop()
  lifecycle = 'stopped'
  return status()
}

async function status () {
  const libp2p = node?.status === 'started' ? node.libp2p : null
  const connections = libp2p?.getConnections() ?? []
  return {
    schema_version: 1,
    implementation: 'helia',
    lifecycle_state: lifecycle,
    peer_id: libp2p?.peerId?.toString() ?? null,
    transports: ['bitswap', 'kademlia', 'websocket', 'webtransport', 'webrtc', 'circuit-relay'],
    listen_addresses: (libp2p?.getMultiaddrs() ?? []).map(address => address.toString()),
    peers: [...new Set(connections.map(connection => connection.remotePeer.toString()))],
    connected_peers: new Set(connections.map(connection => connection.remotePeer.toString())).size,
    routing_status: libp2p == null ? 'stopped' : 'browser_dht_ready',
    storage: 'indexeddb',
    bytes_up: bytesUp,
    bytes_down: bytesDown,
    persists_after_app_close: false,
    limitations: [
      'closing the page stops content serving',
      ...(document.visibilityState === 'hidden'
        ? ['browser background scheduling may suspend network activity']
        : [])
    ],
    last_error: lastError
  }
}

async function connect (address) {
  await start()
  const connection = await node.libp2p.dial(multiaddr(address), {
    signal: AbortSignal.timeout(30_000)
  })
  return {
    connected: true,
    peer_id: connection.remotePeer.toString(),
    remote_address: connection.remoteAddr.toString()
  }
}

async function addBytes (base64, pin = true) {
  await start()
  const bytes = fromBase64(base64)
  const cid = await fs.addBytes(bytes)
  if (pin) await drain(node.pins.add(cid))
  bytesUp += bytes.byteLength
  return { cid: cid.toString(), byte_length: bytes.byteLength, pinned: pin }
}

async function cat (value, maxBytes = MAX_CAT_BYTES) {
  await start()
  const cid = CID.parse(value.replace(/^\/ipfs\//, ''))
  const chunks = []
  let length = 0
  for await (const chunk of fs.cat(cid, { signal: AbortSignal.timeout(30_000) })) {
    length += chunk.byteLength
    if (length > Math.min(maxBytes, MAX_CAT_BYTES)) {
      throw new Error(`content exceeds ${Math.min(maxBytes, MAX_CAT_BYTES)} bytes`)
    }
    chunks.push(chunk)
  }
  const bytes = new Uint8Array(length)
  let offset = 0
  for (const chunk of chunks) {
    bytes.set(chunk, offset)
    offset += chunk.byteLength
  }
  bytesDown += length
  return { cid: cid.toString(), byte_length: length, base64: toBase64(bytes) }
}

async function pin (value) {
  await start()
  const cid = CID.parse(value.replace(/^\/ipfs\//, ''))
  await drain(node.pins.add(cid))
  return { cid: cid.toString(), pinned: true }
}

async function unpin (value) {
  await start()
  const cid = CID.parse(value.replace(/^\/ipfs\//, ''))
  await drain(node.pins.rm(cid))
  return { cid: cid.toString(), pinned: false }
}

async function drain (iterable) {
  for await (const _ of iterable) {
    // Pin APIs are async iterables; consuming them commits the operation.
  }
}

function fromBase64 (value) {
  const binary = atob(value)
  const bytes = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i)
  return bytes
}

function toBase64 (bytes) {
  const chunkSize = 0x8000
  let binary = ''
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + chunkSize))
  }
  return btoa(binary)
}

async function jsonCall (callback) {
  try {
    return JSON.stringify({ ok: true, value: await callback() })
  } catch (error) {
    lastError = String(error)
    return JSON.stringify({ ok: false, error: lastError })
  }
}

window.jimmusicHeliaStart = () => jsonCall(start)
window.jimmusicHeliaStop = () => jsonCall(stop)
window.jimmusicHeliaStatus = () => jsonCall(status)
window.jimmusicHeliaConnect = address => jsonCall(() => connect(address))
window.jimmusicHeliaAddBytes = (base64, pin) => jsonCall(() => addBytes(base64, pin))
window.jimmusicHeliaCat = (cid, maxBytes) => jsonCall(() => cat(cid, maxBytes))
window.jimmusicHeliaPin = cid => jsonCall(() => pin(cid))
window.jimmusicHeliaUnpin = cid => jsonCall(() => unpin(cid))

window.addEventListener('pagehide', () => { void stop() })
window.addEventListener('pageshow', () => { void start() })
document.addEventListener('visibilitychange', () => {
  lifecycle = document.visibilityState === 'hidden' ? 'background_degraded' : 'running'
})
