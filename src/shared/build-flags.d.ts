declare const __SE_CANARY_BUILD__: boolean

declare module '*.mjs' {
  export const CANARY_IDENTIFIER: string
  export const CANARY_PRODUCT_NAME: string
  export const CANARY_UPDATE_MANIFEST_URL: string
  export function resolveCanaryVersion(packageVersion: string, requestedVersion?: string): string
  export function canaryEnvironment(version: string): Record<string, string | undefined>
}
