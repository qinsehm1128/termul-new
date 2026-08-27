import type { DirectoryEntry } from '@shared/types/filesystem.types'
import { ChevronDown, ChevronRight, Loader2 } from 'lucide-react'
import type { ReactNode } from 'react'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { CollapseExpandMotion } from '@/components/ui/collapse-expand-motion'
import { ContextMenu, ContextMenuTrigger } from '@/components/ui/context-menu'
import { usePaneDnd } from '@/hooks/use-pane-dnd'
import { useTreeLongPressDrag } from '@/hooks/use-tree-long-press-drag'
import { cn } from '@/lib/utils'
import { isRejectedMove, useFileExplorerStore } from '@/stores/file-explorer-store'
import { MaterialFileIcon } from './MaterialFileIcon'

/** Distinct from the pane DnD payload so neither drop target claims the other's drag. */
const TREE_MOVE_MIME = 'application/x-termul-tree-move'

interface FileTreeNodeProps {
  entry: DirectoryEntry
  depth: number
  isExpanded: boolean
  isSelected: boolean
  isLoading: boolean
  children?: DirectoryEntry[]
  onToggle: (path: string) => void
  onSelect: (path: string) => void
  onContextMenu: (e: React.MouseEvent, entry: DirectoryEntry) => void
  onClick?: (e: React.MouseEvent, entry: DirectoryEntry) => void
  /**
   * Builds the declarative `<ContextMenuContent>` for this node. When
   * provided, the row is wrapped in `<ContextMenu><ContextMenuTrigger
   * asChild>` so right-click opens the Radix menu at the pointer; the
   * `onContextMenu` prop still seeds selection + stops the global trigger.
   */
  renderContextMenu?: (entry: DirectoryEntry) => ReactNode
}

export function FileTreeNode({
  entry,
  depth,
  isExpanded,
  isSelected,
  isLoading,
  children,
  onToggle,
  onSelect,
  onContextMenu,
  onClick,
  renderContextMenu
}: FileTreeNodeProps): React.JSX.Element {
  const { t } = useTranslation('projects')
  const isDir = entry.type === 'directory'
  const isIgnored = entry.ignored === true
  const suppressTreeAnimations = useFileExplorerStore((state) => state.suppressTreeAnimations)
  const finalizeDirectoryCollapse = useFileExplorerStore((state) => state.finalizeDirectoryCollapse)
  const { startFileDrag } = usePaneDnd()
  const selectedPaths = useFileExplorerStore((state) => state.selectedPaths)
  const dragPaths = useFileExplorerStore((state) => state.dragPaths)
  const beginEntryDrag = useFileExplorerStore((state) => state.beginEntryDrag)
  const endEntryDrag = useFileExplorerStore((state) => state.endEntryDrag)
  const moveEntries = useFileExplorerStore((state) => state.moveEntries)
  const [isDropTarget, setIsDropTarget] = useState(false)
  const [showTooltip, setShowTooltip] = useState(false)
  const tooltipTimerRef = useRef<number | null>(null)

  useEffect(() => {
    return () => {
      if (tooltipTimerRef.current !== null) {
        window.clearTimeout(tooltipTimerRef.current)
      }
    }
  }, [])

  const handleClick = (e: React.MouseEvent): void => {
    // Pass click event if handler provided (for multi-select handling)
    if (onClick) {
      onClick(e, entry)
      return
    }

    if (isDir) {
      onToggle(entry.path)
    } else {
      onSelect(entry.path)
    }
  }

  const handleDragStart = (e: React.DragEvent): void => {
    // Dragging a row inside a multi-selection moves the whole selection, the
    // same rule the context menu's delete already follows.
    const dragged =
      selectedPaths.has(entry.path) && selectedPaths.size > 1
        ? Array.from(selectedPaths)
        : [entry.path]
    beginEntryDrag(dragged)
    e.dataTransfer.setData(TREE_MOVE_MIME, JSON.stringify(dragged))
    e.dataTransfer.effectAllowed = 'move'

    // Files keep their existing "drop onto a pane to open it" payload; the two
    // live under different MIME types so neither drop target sees the other's.
    if (!isDir) startFileDrag(entry.path, e)
  }

  const handleDragEnd = (): void => {
    endEntryDrag()
    setIsDropTarget(false)
  }

  // Touch never fires HTML5 drag events, so the same move is reachable there
  // by holding the row and dragging with a finger.
  const longPress = useTreeLongPressDrag({
    getDragPaths: () =>
      selectedPaths.has(entry.path) && selectedPaths.size > 1
        ? Array.from(selectedPaths)
        : [entry.path],
    onDragStart: beginEntryDrag,
    onDrop: (paths, target) => {
      endEntryDrag()
      void moveEntries(paths, target.path)
    },
    onCancel: endEntryDrag
  })

  const acceptsDrop = isDir && dragPaths.some((path) => !isRejectedMove(path, entry.path))

  const handleDragOver = (e: React.DragEvent): void => {
    if (!acceptsDrop) return
    // dataTransfer is in protected mode during dragover, so the decision has
    // to come from the store payload, not from reading the transfer.
    e.preventDefault()
    e.stopPropagation()
    e.dataTransfer.dropEffect = 'move'
    setIsDropTarget(true)
  }

  const handleDragLeave = (): void => {
    setIsDropTarget(false)
  }

  const handleDrop = (e: React.DragEvent): void => {
    if (!acceptsDrop) return
    e.preventDefault()
    e.stopPropagation()
    const paths = dragPaths
    endEntryDrag()
    setIsDropTarget(false)
    void moveEntries(paths, entry.path)
  }

  const handleMouseEnter = (): void => {
    if (tooltipTimerRef.current !== null) {
      window.clearTimeout(tooltipTimerRef.current)
    }

    tooltipTimerRef.current = window.setTimeout(() => {
      setShowTooltip(true)
    }, 900)
  }

  const handleMouseLeave = (): void => {
    if (tooltipTimerRef.current !== null) {
      window.clearTimeout(tooltipTimerRef.current)
      tooltipTimerRef.current = null
    }
    setShowTooltip(false)
  }

  return (
    <>
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <div
            data-path={entry.path}
            className={cn(
              'group relative flex h-7 min-w-0 cursor-pointer select-none items-center text-sm transition-colors duration-150 ease-[var(--ease-out)]',
              isIgnored && 'opacity-50',
              isDropTarget || longPress.hoverTarget?.path === entry.path
                ? 'bg-primary/15 ring-1 ring-inset ring-primary'
                : isSelected
                  ? 'bg-sidebar-accent text-foreground ring-1 ring-inset ring-primary/35'
                  : 'hover:bg-sidebar-accent/50'
            )}
            title={isIgnored ? t('fileContext.gitIgnored', { name: entry.name }) : undefined}
            style={{ paddingLeft: depth * 16 + 4 }}
            onClick={handleClick}
            onContextMenu={(e) => onContextMenu(e, entry)}
            onMouseEnter={handleMouseEnter}
            onMouseLeave={handleMouseLeave}
            data-entry-type={entry.type}
            draggable
            {...longPress.handlers}
            onDragStart={handleDragStart}
            onDragEnd={handleDragEnd}
            onDragOver={handleDragOver}
            onDragLeave={handleDragLeave}
            onDrop={handleDrop}
          >
            <div className="flex min-w-0 flex-1 items-center overflow-hidden">
              {isDir && (
                <span className="flex-shrink-0 w-4 h-4 flex items-center justify-center mr-0.5">
                  {isLoading ? (
                    <Loader2 size={12} className="animate-spin text-muted-foreground" />
                  ) : isExpanded ? (
                    <ChevronDown size={12} className="text-muted-foreground" />
                  ) : (
                    <ChevronRight size={12} className="text-muted-foreground" />
                  )}
                </span>
              )}
              {!isDir && <span className="w-4 mr-0.5 flex-shrink-0" />}
              <MaterialFileIcon
                name={entry.name}
                extension={entry.extension}
                isDirectory={isDir}
                isExpanded={isExpanded}
                depth={depth}
                size={14}
                className="mr-1.5"
              />
              <span className="min-w-0 flex-1 truncate">{entry.name}</span>
            </div>

            {showTooltip && (
              <div className="pointer-events-none absolute left-2 top-[calc(100%+2px)] z-50 max-w-[420px] rounded border border-border bg-popover px-2 py-1 text-xs text-popover-foreground shadow-lg">
                {entry.name}
              </div>
            )}
          </div>
        </ContextMenuTrigger>
        {renderContextMenu?.(entry)}
      </ContextMenu>

      {isDir &&
        (suppressTreeAnimations ? (
          isExpanded &&
          children?.map((child) => (
            <FileTreeNodeWrapper
              key={child.path}
              entry={child}
              depth={depth + 1}
              onToggle={onToggle}
              onSelect={onSelect}
              onContextMenu={onContextMenu}
              onClick={onClick}
              renderContextMenu={renderContextMenu}
            />
          ))
        ) : (
          <CollapseExpandMotion
            open={isExpanded}
            onExitComplete={() => finalizeDirectoryCollapse(entry.path)}
          >
            {children?.map((child) => (
              <FileTreeNodeWrapper
                key={child.path}
                entry={child}
                depth={depth + 1}
                onToggle={onToggle}
                onSelect={onSelect}
                onContextMenu={onContextMenu}
                onClick={onClick}
                renderContextMenu={renderContextMenu}
              />
            ))}
          </CollapseExpandMotion>
        ))}
    </>
  )
}

interface FileTreeNodeWrapperProps {
  entry: DirectoryEntry
  depth: number
  onToggle: (path: string) => void
  onSelect: (path: string) => void
  onContextMenu: (e: React.MouseEvent, entry: DirectoryEntry) => void
  onClick?: (e: React.MouseEvent, entry: DirectoryEntry) => void
  renderContextMenu?: (entry: DirectoryEntry) => ReactNode
}

function FileTreeNodeWrapper({
  entry,
  depth,
  onToggle,
  onSelect,
  onContextMenu,
  onClick,
  renderContextMenu
}: FileTreeNodeWrapperProps): React.JSX.Element {
  const isExpanded = useFileExplorerStore((state) => state.expandedDirs.has(entry.path))
  const isSelected = useFileExplorerStore((state) => state.selectedPaths.has(entry.path))
  const isLoading = useFileExplorerStore((state) => state.loadingDirs.has(entry.path))
  const children = useFileExplorerStore((state) => state.directoryContents.get(entry.path))

  return (
    <FileTreeNode
      entry={entry}
      depth={depth}
      isExpanded={isExpanded}
      isSelected={isSelected}
      isLoading={isLoading}
      // biome-ignore lint/correctness/noChildrenProp: `children` is a typed directory-data prop, not React children
      children={children}
      onToggle={onToggle}
      onSelect={onSelect}
      onContextMenu={onContextMenu}
      onClick={onClick}
      renderContextMenu={renderContextMenu}
    />
  )
}

export { FileTreeNodeWrapper }
