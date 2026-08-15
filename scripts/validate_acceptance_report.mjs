#!/usr/bin/env node

import fs from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const PLATFORMS = ['windows', 'macos', 'linux', 'android', 'ios', 'harmonyos', 'web']
const REQUIRED_SCENARIOS = [
  'install_and_first_launch',
  'offline_library_and_playlist',
  'local_playback_seek_and_queue_resume',
  'network_fetch_without_public_gateway',
  'content_tamper_rejected',
  'plugin_install_rollback_and_revoke',
  'wasm_capability_and_resource_isolation',
  'upgrade_and_schema_recovery',
  'forced_termination_recovery',
  'background_lifecycle',
  'accessibility',
  'diagnostics_privacy',
]
const REQUIRED_RESOURCES = [
  'startup_ms',
  'peak_memory_mib',
  'energy_wh_per_hour',
  'network_mib_per_hour',
]
const SHA256 = /^[a-f0-9]{64}$/
const COMMIT = /^[a-f0-9]{40}$/

const scriptDir = path.dirname(fileURLToPath(import.meta.url))
const repositoryRoot = path.resolve(scriptDir, '..')

function fail(message) {
  throw new Error(message)
}

function nonEmptyString(value, field, minLength = 1) {
  if (typeof value !== 'string' || value.trim().length < minLength) {
    fail(`${field} must be a non-empty string`)
  }
}

function exactKeys(object, required, field) {
  if (!object || typeof object !== 'object' || Array.isArray(object)) {
    fail(`${field} must be an object`)
  }
  const missing = required.filter((key) => !(key in object))
  if (missing.length > 0) fail(`${field} is missing: ${missing.join(', ')}`)
}

export function readP0Requirements(markdownPath = path.join(repositoryRoot, 'JimMusic需求文档.md')) {
  const rows = fs.readFileSync(markdownPath, 'utf8')
    .split(/\r?\n/u)
    .map((line) => line.match(/^\|\s*([A-Z]+-\d+)\s*\|\s*(P\d)\s*\|\s*([^|]+?)\s*\|/u))
    .filter(Boolean)
    .filter((match) => match[2] === 'P0')
    .map((match) => ({ id: match[1], scope: match[3].trim() }))

  const ids = new Set(rows.map(({ id }) => id))
  if (rows.length !== ids.size) fail('the requirements document contains duplicate P0 IDs')
  if (rows.length < 100) fail(`unexpectedly small P0 catalog: ${rows.length}`)
  return rows
}

function scopeDisposition(scope, platform) {
  if (scope === 'ALL') return 'required'
  if (scope === '支持平台') return 'supported-or-explicitly-unsupported'
  if (scope === '原生平台') return platform === 'web' ? 'not-applicable' : 'required'
  if (scope === '桌面') return ['windows', 'macos', 'linux'].includes(platform) ? 'required' : 'not-applicable'

  const token = {
    WIN: 'windows', MAC: 'macos', LNX: 'linux', AND: 'android',
    IOS: 'ios', HOS: 'harmonyos', WEB: 'web',
  }
  const platforms = scope.split('/').map((part) => token[part.trim()]).filter(Boolean)
  if (platforms.length === 0) fail(`unknown requirement scope: ${scope}`)
  return platforms.includes(platform) ? 'required' : 'not-applicable'
}

function validateEvidence(value, field) {
  exactKeys(value, ['uri', 'sha256'], field)
  nonEmptyString(value.uri, `${field}.uri`)
  if (!SHA256.test(value.sha256)) fail(`${field}.sha256 must be a lowercase SHA-256 digest`)
}

function validateP0(report, catalog) {
  exactKeys(report.p0, ['result', 'exemptions', 'requirements'], 'p0')
  if (report.p0.result !== 'pass') fail('p0.result must be pass')
  if (!Array.isArray(report.p0.exemptions) || report.p0.exemptions.length !== 0) {
    fail('p0.exemptions must be empty; stable candidates do not permit platform P0 exemptions')
  }
  if (!Array.isArray(report.p0.requirements)) fail('p0.requirements must be an array')

  const results = new Map()
  for (const [index, item] of report.p0.requirements.entries()) {
    exactKeys(item, ['id', 'result', 'evidence'], `p0.requirements[${index}]`)
    if (results.has(item.id)) fail(`duplicate P0 result: ${item.id}`)
    if (!['pass', 'unsupported'].includes(item.result)) fail(`${item.id} has invalid result ${item.result}`)
    if (!Array.isArray(item.evidence) || item.evidence.length === 0) fail(`${item.id} must contain evidence`)
    item.evidence.forEach((evidence, evidenceIndex) => validateEvidence(evidence, `${item.id}.evidence[${evidenceIndex}]`))
    if (item.result === 'unsupported') nonEmptyString(item.reason, `${item.id}.reason`, 20)
    results.set(item.id, item)
  }

  for (const requirement of catalog) {
    const disposition = scopeDisposition(requirement.scope, report.platform)
    const result = results.get(requirement.id)
    if (disposition === 'not-applicable') {
      if (result) fail(`${requirement.id} is outside ${report.platform} scope and must not be reported`)
      continue
    }
    if (!result) fail(`missing P0 result for ${requirement.id}`)
    if (disposition === 'required' && result.result !== 'pass') {
      fail(`${requirement.id} is required on ${report.platform} and cannot be marked unsupported`)
    }
    results.delete(requirement.id)
  }
  if (results.size > 0) fail(`unknown or inapplicable P0 IDs: ${[...results.keys()].join(', ')}`)
}

function validateScenarios(report) {
  if (!Array.isArray(report.scenarios)) fail('scenarios must be an array')
  const scenarios = new Map()
  for (const [index, scenario] of report.scenarios.entries()) {
    exactKeys(scenario, ['id', 'result', 'evidence'], `scenarios[${index}]`)
    if (scenarios.has(scenario.id)) fail(`duplicate scenario: ${scenario.id}`)
    if (scenario.result !== 'pass') fail(`scenario ${scenario.id} did not pass`)
    if (!Array.isArray(scenario.evidence) || scenario.evidence.length === 0) {
      fail(`scenario ${scenario.id} must contain evidence`)
    }
    scenario.evidence.forEach((evidence, evidenceIndex) => validateEvidence(evidence, `${scenario.id}.evidence[${evidenceIndex}]`))
    scenarios.set(scenario.id, scenario)
  }
  const missing = REQUIRED_SCENARIOS.filter((id) => !scenarios.has(id))
  if (missing.length > 0) fail(`missing acceptance scenarios: ${missing.join(', ')}`)
}

function validateResources(report) {
  if (!Array.isArray(report.resources)) fail('resources must be an array')
  const resources = new Map()
  for (const [index, metric] of report.resources.entries()) {
    exactKeys(metric, ['name', 'baseline', 'candidate', 'unit', 'samples', 'evidence'], `resources[${index}]`)
    if (resources.has(metric.name)) fail(`duplicate resource metric: ${metric.name}`)
    if (!Number.isFinite(metric.baseline) || !Number.isFinite(metric.candidate) || !(metric.baseline > 0) || !(metric.candidate >= 0)) {
      fail(`${metric.name} values must be finite and baseline must be positive`)
    }
    if (!Number.isInteger(metric.samples) || metric.samples < 5) fail(`${metric.name}.samples must be at least 5`)
    nonEmptyString(metric.unit, `${metric.name}.unit`)
    validateEvidence(metric.evidence, `${metric.name}.evidence`)
    const regression = ((metric.candidate - metric.baseline) / metric.baseline) * 100
    if (regression > 15.000001) fail(`${metric.name} regressed by ${regression.toFixed(2)}%, above the 15% gate`)
    resources.set(metric.name, metric)
  }
  const missing = REQUIRED_RESOURCES.filter((name) => !resources.has(name))
  if (missing.length > 0) fail(`missing M0 resource metrics: ${missing.join(', ')}`)
}

function validateAudioClaims(report) {
  if (!Array.isArray(report.audio_capabilities) || report.audio_capabilities.length === 0) {
    fail('audio_capabilities must explicitly document at least one supported or unsupported path')
  }
  for (const [index, claim] of report.audio_capabilities.entries()) {
    exactKeys(claim, ['capability', 'declaration', 'reason'], `audio_capabilities[${index}]`)
    nonEmptyString(claim.capability, `audio_capabilities[${index}].capability`)
    nonEmptyString(claim.reason, `audio_capabilities[${index}].reason`, 20)
    if (!['supported', 'unsupported'].includes(claim.declaration)) fail(`invalid audio declaration for ${claim.capability}`)
    if (claim.declaration === 'supported') {
      exactKeys(claim, ['device', 'driver', 'negotiated_format', 'evidence'], `audio_capabilities[${index}]`)
      nonEmptyString(claim.device, `${claim.capability}.device`)
      nonEmptyString(claim.driver, `${claim.capability}.driver`)
      nonEmptyString(claim.negotiated_format, `${claim.capability}.negotiated_format`)
      validateEvidence(claim.evidence, `${claim.capability}.evidence`)
    }
  }
}

export function validateReport(report, options = {}) {
  exactKeys(report, [
    'schema_version', 'candidate', 'platform', 'generated_at', 'runner', 'p0',
    'scenarios', 'resources', 'audio_capabilities',
  ], 'report')
  if (report.schema_version !== 1) fail('schema_version must be 1')
  if (!PLATFORMS.includes(report.platform)) fail(`unknown platform: ${report.platform}`)
  if (options.platform && report.platform !== options.platform) fail(`expected ${options.platform}, got ${report.platform}`)
  if (Number.isNaN(Date.parse(report.generated_at))) fail('generated_at must be an RFC 3339 timestamp')

  exactKeys(report.candidate, ['version', 'commit', 'artifact_sha256'], 'candidate')
  nonEmptyString(report.candidate.version, 'candidate.version')
  if (!COMMIT.test(report.candidate.commit)) fail('candidate.commit must be a lowercase 40-character Git commit')
  if (options.commit && report.candidate.commit !== options.commit) fail('candidate.commit does not match the workflow commit')
  if (!SHA256.test(report.candidate.artifact_sha256)) fail('candidate.artifact_sha256 must be a lowercase SHA-256 digest')

  exactKeys(report.runner, ['id', 'os_version', 'device_model', 'physical_device'], 'runner')
  nonEmptyString(report.runner.id, 'runner.id')
  nonEmptyString(report.runner.os_version, 'runner.os_version')
  nonEmptyString(report.runner.device_model, 'runner.device_model')
  if (report.runner.physical_device !== true) fail('runner.physical_device must be true for release evidence')

  const catalog = options.catalog ?? readP0Requirements()
  validateP0(report, catalog)
  validateScenarios(report)
  validateResources(report)
  validateAudioClaims(report)
  return report
}

export function validateFiles(files, options = {}) {
  if (files.length === 0) fail('at least one report path is required')
  const reports = files.map((file) => {
    const report = JSON.parse(fs.readFileSync(file, 'utf8'))
    return validateReport(report, options.platform ? options : { ...options, platform: undefined })
  })
  if (options.requireAllPlatforms) {
    const platforms = new Set(reports.map(({ platform }) => platform))
    const missing = PLATFORMS.filter((platform) => !platforms.has(platform))
    const duplicates = reports.map(({ platform }) => platform).filter((platform, index, all) => all.indexOf(platform) !== index)
    if (missing.length > 0 || duplicates.length > 0 || reports.length !== PLATFORMS.length) {
      fail(`expected one report for every platform; missing=${missing.join(',') || 'none'} duplicates=${[...new Set(duplicates)].join(',') || 'none'}`)
    }
    const commits = new Set(reports.map(({ candidate }) => candidate.commit))
    if (commits.size !== 1) fail('all platform reports must reference the same candidate commit')
  }
  return reports
}

function parseArguments(argv) {
  const options = { files: [], requireAllPlatforms: false }
  for (let index = 0; index < argv.length; index += 1) {
    const value = argv[index]
    if (value === '--commit') options.commit = argv[++index]
    else if (value === '--platform') options.platform = argv[++index]
    else if (value === '--require-all-platforms') options.requireAllPlatforms = true
    else options.files.push(value)
  }
  if (options.commit && !COMMIT.test(options.commit)) fail('--commit must be a lowercase 40-character Git commit')
  if (options.platform && !PLATFORMS.includes(options.platform)) fail(`--platform must be one of ${PLATFORMS.join(', ')}`)
  return options
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const options = parseArguments(process.argv.slice(2))
    const reports = validateFiles(options.files, options)
    process.stdout.write(`validated ${reports.length} acceptance report(s): ${reports.map(({ platform }) => platform).join(', ')}\n`)
  } catch (error) {
    process.stderr.write(`acceptance report rejected: ${error.message}\n`)
    process.exitCode = 1
  }
}
