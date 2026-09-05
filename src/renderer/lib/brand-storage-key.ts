import { brandCanonical } from '@shared/brand'

/**
 * A brand-prefixed web-storage key, plus the pre-rename spelling to read from.
 *
 * These keys predate the `storageKeyPrefix` contract and were spelled with the
 * brand glued on by hand (`termul.gitDiffViewMode`, `termul-ssh-panel-height`),
 * so `acceptedBrandValues('storageKeyPrefix')` cannot reconstruct them — it
 * only knows the `termul:` form. Writing the flipped key without carrying the
 * old one would silently reset a preference the user set, which the migration
 * charter treats as data loss even when the datum is only a panel height.
 *
 * `legacy` is the exact string already in the user's storage; pass it verbatim.
 */
export interface BrandedStorageKey {
  /** The key written from now on. */
  readonly canonical: string
  /** The key to fall back to on read. Never written again. */
  readonly legacy: string
}

/** `<prefix><suffix>` under today's brand, with `legacy` kept for reads. */
export function brandedStorageKey(suffix: string, legacy: string): BrandedStorageKey {
  return { canonical: `${brandCanonical().storageKeyPrefix}${suffix}`, legacy }
}

/**
 * Read `key.canonical`, falling back to `key.legacy`.
 *
 * Deliberately does not migrate on read: a write happens when the user next
 * changes the setting, and rewriting storage as a side effect of a read makes
 * a getter surprising.
 */
export function readBrandedStorage(
  storage: Pick<Storage, 'getItem'>,
  key: BrandedStorageKey
): string | null {
  return storage.getItem(key.canonical) ?? storage.getItem(key.legacy)
}
