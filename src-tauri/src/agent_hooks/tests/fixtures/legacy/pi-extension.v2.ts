// Se managed agent hooks v2
import { spawn } from "node:child_process";
export default function (pi: any) {
  // session_start precedes the CLI's initial prompt/agent_start. Unknown flags,
  // positional prompts and @file inputs therefore fail closed, not briefly idle.
  const flags = new Set(["--continue", "-c", "--resume", "-r", "--no-session", "--no-tools", "--no-extensions", "-ne", "--no-skills", "-ns", "--no-prompt-templates", "-np", "--no-themes", "--offline", "--hide-cwd-in-prompt", "--verbose"]);
  const options = new Set(["--provider", "--model", "--api-key", "--thinking", "--session", "--session-dir", "--tools", "--extension", "-e", "--skill", "--prompt-template", "--theme", "--models", "--keybindings", "--system-prompt", "--append-system-prompt"]);
  const args = process.argv.slice(2);
  let promptFree = true;
  for (let i = 0; i < args.length; i++) {
    if (flags.has(args[i])) continue;
    if (options.has(args[i]) && args[i + 1] && !args[i + 1].startsWith("-")) { i++; continue; }
    promptFree = false;
    break;
  }
  let revision = 0;
  let delivery = Promise.resolve();
  const report = (name: string, event: any, ctx: any, ready: any = undefined) => {
    try {
      const payload = JSON.stringify({ event: name, session_id: ctx?.sessionManager?.getSessionId?.(), session_file: ctx?.sessionManager?.getSessionFile?.(), tool_name: event.toolName, ok: !event.isError });
      if (Buffer.byteLength(payload) > 1048576) return;
      // Delivery is detached from pi callbacks, but ordered: a late idle POST
      // must not overtake agent_start or arrive before its session identity.
      delivery = delivery.then(() => new Promise<void>((done) => {
        try {
          if (ready && !ready()) { done(); return; }
          const child = spawn("/se-hooks-legacy-fixture/home 'with spaces/.se-manager/agent-hooks/pi-hook.sh", [], { stdio: ["pipe", "ignore", "ignore"] });
          child.on("error", () => done());
          child.on("close", () => done());
          child.stdin.on("error", () => {});
          child.stdin.end(payload);
          child.unref();
        } catch { done(); }
      }));
    } catch { /* Observers must never alter tool decisions or interrupt pi. */ }
  };
  const events = ["session_start", "agent_start", "agent_end", "tool_execution_start", "tool_execution_end", "tool_approval_requested", "tool_approval_resolved", "ui_prompt_start", "ui_prompt_end", "session_shutdown"];
  for (const name of events) pi.on(name, (event: any, ctx: any) => {
    if (!process.env.SE_TERMINAL_ID) return;
    const current = ++revision;
    report(name, event, ctx);
    if (name === "session_start") report("idle", event, ctx, () =>
      current === revision && promptFree && process.stdin.isTTY === true &&
      ctx?.hasUI === true && ctx?.isIdle?.() === true && ctx?.hasPendingMessages?.() === false);
  });
}
