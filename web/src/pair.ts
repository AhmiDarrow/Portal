// The screen a device that is not paired shows: a number to type into
// Portal on the PC. Once the PC accepts it, the page loads as usual.

import { api } from "./api";

const POLL_MS = 2000;
const RETRY_MS = 30_000;
let showing = false;

export function showPairing(): void {
  if (showing) return;
  showing = true;
  document.title = "Pair this device · Portal";
  document.body.innerHTML = `
    <main class="pair" aria-labelledby="pair-title">
      <h1 id="pair-title">Pair this device</h1>
      <p class="pair-code" aria-live="polite">······</p>
      <p>On the PC running Portal, open <strong>Server</strong> and type this number under <strong>Devices</strong>.</p>
      <p class="muted pair-note" role="status"></p>
    </main>
  `;
  void run();
}

async function run(): Promise<void> {
  const code = document.querySelector<HTMLElement>(".pair-code");
  const note = document.querySelector<HTMLElement>(".pair-note");
  for (;;) {
    let ticket = "";
    try {
      const started = await api.pairStart();
      ticket = started.ticket;
      if (code) code.textContent = started.code.replace(/^(\d{3})(\d{3})$/, "$1 $2");
      if (note) note.textContent = `The number works for ${Math.round(started.expires_in / 60)} minutes.`;
    } catch (err) {
      if (note) note.textContent = err instanceof Error ? err.message : "Portal did not answer.";
      await sleep(RETRY_MS);
      continue;
    }
    for (;;) {
      await sleep(POLL_MS);
      let state = "waiting";
      try {
        state = (await api.pairStatus(ticket)).state;
      } catch {
        continue;
      }
      if (state === "paired") {
        location.reload();
        return;
      }
      if (state === "expired") break;
    }
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}
