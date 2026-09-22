import process from 'node:process'

export const CANARY_IDENTIFIER = 'com.se-manager.app.canary'
export const CANARY_PRODUCT_NAME = 'Se Manager Canary'
export const CANARY_UPDATE_MANIFEST_URL =
  'https://github.com/qinsehm1128/termul-new/releases/download/canary/latest-canary.json'

export function resolveCanaryVersion(
  packageVersion,
  requestedVersion = process.env.CANARY_VERSION
) {
  const version = requestedVersion?.trim() || `${packageVersion}-canary.1`
  if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) {
    throw new Error(`Invalid Canary version: ${version}`)
  }
  return version
}

export function canaryEnvironment(version) {
  return {
    ...process.env,
    SE_CANARY_BUILD: '1',
    VITE_SE_CANARY: '1',
    VITE_APP_VERSION_OVERRIDE: version
  }
}
