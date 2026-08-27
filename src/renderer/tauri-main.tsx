import '@fontsource-variable/geist/index.css'
import '@fontsource-variable/inter/index.css'
import '@fontsource-variable/jetbrains-mono/index.css'
import { createRoot } from 'react-dom/client'
import { bootstrapI18n } from '@/i18n/bootstrap'
import { installBootInstrumentation } from './boot-instrumentation'
import TauriApp from './TauriApp'
import './index.css'

// This is the entry `tauri-index.html` loads, i.e. the only one the packaged
// desktop app runs. Instrumentation that lives solely in `main.tsx` ships in
// the bundle but never executes here — see `boot-instrumentation.ts`.
installBootInstrumentation()

async function bootstrap(): Promise<void> {
  await bootstrapI18n()
  createRoot(document.getElementById('root')!).render(<TauriApp />)
}

void bootstrap()
