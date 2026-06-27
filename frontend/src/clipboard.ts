/** Copy `text` to the clipboard, returning whether it succeeded.
 *
 * `navigator.clipboard` only exists in a **secure context** (HTTPS or
 * `localhost`). Bifrost is usually reached over plain `http://<lan-ip>:3000`,
 * where `navigator.clipboard` is `undefined` — so the modern API is tried first
 * and we fall back to a hidden-textarea `execCommand("copy")` that works on
 * insecure origins too. */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // Secure-context API present but blocked (permissions/focus) — fall through.
  }
  try {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.position = "fixed";
    ta.style.top = "-9999px";
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand("copy");
    document.body.removeChild(ta);
    return ok;
  } catch {
    return false;
  }
}
