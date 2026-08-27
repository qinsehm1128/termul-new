import { lazy, Suspense } from 'react'
import { createHashRouter, type RouteObject } from 'react-router-dom'
import { ChatRoute } from '@/components/ChatRoute'
import { ConversationRoute } from '@/components/conversation/ConversationRoute'
import { Skeleton } from '@/components/ui/skeleton'
import WorkspaceLayout from '@/layouts/WorkspaceLayout'

const WorkspaceDashboard = lazy(() => import('@/pages/WorkspaceDashboard'))
const TerminalBoard = lazy(() => import('@/pages/TerminalBoard'))
const ProjectSettings = lazy(() => import('@/pages/ProjectSettings'))
const AppPreferences = lazy(() => import('@/pages/AppPreferences'))
const WorkspaceSnapshots = lazy(() => import('@/pages/WorkspaceSnapshots'))
const ScheduledTasks = lazy(() => import('@/pages/ScheduledTasks'))
const NotFound = lazy(() => import('@/pages/NotFound'))

function RouteFallback(): React.JSX.Element {
  return <Skeleton className="h-full w-full" />
}

function deferred(element: React.JSX.Element): React.JSX.Element {
  return <Suspense fallback={<RouteFallback />}>{element}</Suspense>
}

function CompanionTerminalRoute(): null {
  return null
}

/** The portable route table used by the browser and native renderer roots. */
export const portableRouteObjects: RouteObject[] = [
  {
    path: '/',
    element: <WorkspaceLayout />,
    children: [
      // The index route owns the regular project workspace (terminal panes);
      // the independent Conversation area is entered explicitly from the
      // Activity Rail chat toggle.
      { path: 'conversations', element: deferred(<WorkspaceDashboard />) },
      { path: 'terminals', element: deferred(<TerminalBoard />) },
      { path: 'c/:conversationId', element: <ConversationRoute /> },
      {
        path: 'legacy/session/:legacyValue',
        element: <ChatRoute sourceKind="legacyAgentSessionId" />
      },
      {
        path: 'legacy/storage/:legacyValue',
        element: <ChatRoute sourceKind="legacyStorageKey" />
      },
      {
        path: 'legacy/history/:legacyValue',
        element: <ChatRoute sourceKind="legacyChatHistoryId" />
      },
      {
        // iOS companion deep-link: focus or create a terminal tab inside the
        // existing responsive workspace without introducing a second shell.
        path: 'terminal',
        element: <CompanionTerminalRoute />
      },
      { path: 'snapshots', element: deferred(<WorkspaceSnapshots />) },
      { path: 'scheduled-tasks', element: deferred(<ScheduledTasks />) },
      { path: 'settings', element: deferred(<ProjectSettings />) },
      { path: 'preferences', element: deferred(<AppPreferences />) }
    ]
  },
  { path: '*', element: deferred(<NotFound />) }
]

export function createPortableRouter(): ReturnType<typeof createHashRouter> {
  return createHashRouter(portableRouteObjects, {
    future: {
      v7_relativeSplatPath: true
    }
  })
}
