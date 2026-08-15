import assert from 'node:assert/strict'
import test from 'node:test'

import { readP0Requirements, validateReport } from '../validate_acceptance_report.mjs'

const digest = 'a'.repeat(64)
const evidence = { uri: 'evidence://fixture', sha256: digest }
const catalog = [
  { id: 'ALL-001', scope: 'ALL' },
  { id: 'WEB-001', scope: 'WEB' },
  { id: 'BPT-001', scope: '支持平台' },
]

function fixture() {
  return {
    schema_version: 1,
    candidate: { version: '2.0.0-rc.1', commit: 'b'.repeat(40), artifact_sha256: digest },
    platform: 'web',
    generated_at: '2026-08-15T00:00:00Z',
    runner: { id: 'browser-lab-1', os_version: 'Test OS', device_model: 'Physical lab host', physical_device: true },
    p0: {
      result: 'pass',
      exemptions: [],
      requirements: [
        { id: 'ALL-001', result: 'pass', evidence: [evidence] },
        { id: 'WEB-001', result: 'pass', evidence: [evidence] },
        { id: 'BPT-001', result: 'unsupported', reason: 'The browser exposes no exclusive hardware session.', evidence: [evidence] },
      ],
    },
    scenarios: [
      'install_and_first_launch', 'offline_library_and_playlist',
      'local_playback_seek_and_queue_resume', 'network_fetch_without_public_gateway',
      'content_tamper_rejected', 'plugin_install_rollback_and_revoke',
      'wasm_capability_and_resource_isolation', 'upgrade_and_schema_recovery',
      'forced_termination_recovery', 'background_lifecycle', 'accessibility',
      'diagnostics_privacy',
    ].map((id) => ({ id, result: 'pass', evidence: [evidence] })),
    resources: [
      ['startup_ms', 'ms'], ['peak_memory_mib', 'MiB'],
      ['energy_wh_per_hour', 'Wh/h'], ['network_mib_per_hour', 'MiB/h'],
    ].map(([name, unit]) => ({ name, unit, baseline: 100, candidate: 115, samples: 5, evidence })),
    audio_capabilities: [{
      capability: 'exclusive-output',
      declaration: 'unsupported',
      reason: 'The browser runtime does not expose an exclusive output API.',
    }],
  }
}

test('loads every P0 row from the product requirements document', () => {
  assert.equal(readP0Requirements().length, 134)
})

test('accepts a complete report with an explicit supported-scope rejection', () => {
  assert.equal(validateReport(fixture(), { catalog }).platform, 'web')
})

test('rejects a resource regression above fifteen percent', () => {
  const report = fixture()
  report.resources[0].candidate = 115.01
  assert.throws(() => validateReport(report, { catalog }), /above the 15% gate/)
})

test('rejects a required P0 requirement marked unsupported', () => {
  const report = fixture()
  report.p0.requirements[0] = {
    id: 'ALL-001', result: 'unsupported', reason: 'This cannot be exempted on any supported release platform.', evidence: [evidence],
  }
  assert.throws(() => validateReport(report, { catalog }), /cannot be marked unsupported/)
})

test('rejects emulator evidence', () => {
  const report = fixture()
  report.runner.physical_device = false
  assert.throws(() => validateReport(report, { catalog }), /physical_device must be true/)
})
