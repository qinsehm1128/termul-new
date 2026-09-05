import {
  ArrowLeftToLine,
  ArrowRightToLine,
  Copy,
  CopyX,
  Edit2,
  Skull,
  X,
  XCircle
} from 'lucide-react'
import type { ReactNode } from 'react'
import { useTranslation } from 'react-i18next'
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger
} from '@/components/ui/context-menu'

/**
 * Shared context-menu wrapper for every workspace tab kind.
 *
 * Each tab surface wraps its tab element in `<TabContextMenu kind="…">`; the
 * `kind` selects the declarative item set rendered inside a single Radix
 * `<ContextMenuContent>`. This dedupes the 7 bespoke `{x,y}` tab menus into
 * one source of truth so every tab shares identical chrome, keyboard
 * navigation, viewport-aware positioning, and touch long-press parity.
 *
 * Per spec: tab menus carry NO shortcut labels (shortcuts are a general-menu
 * concern) — icons use the canonical `mr-2 h-4 w-4` left-of-label convention.
 */
export type TabContextMenuKind =
  | 'terminal'
  | 'editor'
  | 'browser'
  | 'git'
  | 'git-history'
  | 'agent-chat'

/**
 * The bulk-close actions are scoped to the tab *kind* whose menu was opened —
 * "Close all" on a terminal tab closes terminals, not the editor sitting beside
 * it in the same pane.
 *
 * A pane's tab list is mixed, unlike a browser's, so the literal reading of
 * these labels would let one click on a terminal tab kill an unsaved editor or
 * a browser session the user never had in mind. Same-kind scoping is the only
 * reading where nothing is destroyed by surprise, and it is what the existing
 * editor "Close all" already did before these were added.
 */
export interface TabBulkCloseHandlers {
  /** Close every same-kind tab to the left of this one. */
  onCloseLeft?: () => void
  /** Close every same-kind tab to the right of this one. */
  onCloseRight?: () => void
  /** Close every other same-kind tab in this pane. */
  onCloseOthers?: () => void
  /** Close every same-kind tab in this pane. */
  onCloseAll?: () => void
}

interface TabContextMenuProps extends TabBulkCloseHandlers {
  kind: TabContextMenuKind
  /** Primary close action (every kind). */
  onClose: () => void
  /** Terminal rename. */
  onRename?: () => void
  /** Terminal kill (destructive styling). */
  onKill?: () => void
  /** Editor: copy the file path. */
  onCopyPath?: () => void
  /** Disable close/kill while a close is already in flight (terminal). */
  isClosing?: boolean
  /** The tab element to wrap (`asChild` trigger). */
  children: ReactNode
}

export function TabContextMenu({
  kind,
  onClose,
  onRename,
  onKill,
  onCloseLeft,
  onCloseRight,
  onCloseOthers,
  onCloseAll,
  onCopyPath,
  isClosing = false,
  children
}: TabContextMenuProps): React.JSX.Element {
  const { t } = useTranslation('workspace')
  const closeDisabled = isClosing

  /**
   * The bulk block, identical for every kind that has one.
   *
   * `undefined` for an action means "there is nothing on that side" — the tab
   * bar decides, because only it knows the tab's position. A disabled item
   * would be equally honest but adds four permanently-greyed rows to the menu
   * of the first and last tab, which is most of the time.
   */
  const bulkItems = (onCloseLeft || onCloseRight || onCloseOthers || onCloseAll) && (
    <>
      <ContextMenuSeparator />
      {onCloseLeft && (
        <ContextMenuItem onSelect={onCloseLeft}>
          <ArrowLeftToLine className="mr-2 h-4 w-4" /> {t('tabs.closeLeft')}
        </ContextMenuItem>
      )}
      {onCloseRight && (
        <ContextMenuItem onSelect={onCloseRight}>
          <ArrowRightToLine className="mr-2 h-4 w-4" /> {t('tabs.closeRight')}
        </ContextMenuItem>
      )}
      {onCloseOthers && (
        <ContextMenuItem onSelect={onCloseOthers}>
          <CopyX className="mr-2 h-4 w-4" /> {t('tabs.closeOthers')}
        </ContextMenuItem>
      )}
      {onCloseAll && (
        <ContextMenuItem onSelect={onCloseAll}>
          <XCircle className="mr-2 h-4 w-4" /> {t('tabs.closeAll')}
        </ContextMenuItem>
      )}
    </>
  )

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>{children}</ContextMenuTrigger>
      <ContextMenuContent className="w-44">
        {kind === 'terminal' && (
          <>
            {onRename && (
              <ContextMenuItem onSelect={onRename}>
                <Edit2 className="mr-2 h-4 w-4" /> {t('tabs.rename')}
              </ContextMenuItem>
            )}
            <ContextMenuItem onSelect={onClose} disabled={closeDisabled}>
              <X className="mr-2 h-4 w-4" /> {t('tabs.close')}
            </ContextMenuItem>
            {onKill && (
              <ContextMenuItem variant="destructive" onSelect={onKill} disabled={closeDisabled}>
                <Skull className="mr-2 h-4 w-4" /> {t('tabs.killProcess')}
              </ContextMenuItem>
            )}
            {bulkItems}
          </>
        )}

        {kind === 'editor' && (
          <>
            <ContextMenuItem onSelect={onClose}>
              <X className="mr-2 h-4 w-4" /> {t('tabs.close')}
            </ContextMenuItem>
            {bulkItems}
            {onCopyPath && (
              <>
                <ContextMenuSeparator />
                <ContextMenuItem onSelect={onCopyPath}>
                  <Copy className="mr-2 h-4 w-4" /> {t('tabs.copyPath')}
                </ContextMenuItem>
              </>
            )}
          </>
        )}

        {(kind === 'browser' ||
          kind === 'git' ||
          kind === 'git-history' ||
          kind === 'agent-chat') && (
          <ContextMenuItem onSelect={onClose}>
            <X className="mr-2 h-4 w-4" /> {t('tabs.close')}
          </ContextMenuItem>
        )}
      </ContextMenuContent>
    </ContextMenu>
  )
}
