import {
  Bell,
  BookOpen,
  Bug,
  Calculator,
  Calendar,
  ChartBar,
  Clock,
  Cloud,
  Database,
  Folder,
  GitBranch,
  Globe,
  Key,
  ListChecks,
  type LucideIcon,
  MessageSquare,
  Package,
  Play,
  Puzzle,
  Rocket,
  Search,
  Settings,
  Shield,
  Terminal,
  Wrench
} from 'lucide-react'

/** Host-owned components only: plugin input is never rendered as image markup. */
export const pluginViewIcons: Record<string, LucideIcon> = {
  puzzle: Puzzle,
  calculator: Calculator,
  'chart-bar': ChartBar,
  terminal: Terminal,
  database: Database,
  cloud: Cloud,
  'git-branch': GitBranch,
  bug: Bug,
  'book-open': BookOpen,
  bell: Bell,
  calendar: Calendar,
  clock: Clock,
  folder: Folder,
  globe: Globe,
  key: Key,
  'list-checks': ListChecks,
  'message-square': MessageSquare,
  package: Package,
  play: Play,
  rocket: Rocket,
  search: Search,
  settings: Settings,
  shield: Shield,
  wrench: Wrench
}

export function getPluginViewIcon(name?: string): LucideIcon {
  return (name && Object.hasOwn(pluginViewIcons, name) && pluginViewIcons[name]) || Puzzle
}
