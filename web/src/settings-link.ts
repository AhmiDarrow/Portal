// Pure routing for add-on settings rows. No DOM.

export function plainSettingLabel(value: string): string {
  const text = value.replaceAll(/\[CR\]/gi, "\n").replaceAll(/\[TABS\]/gi, " ");
  let out = "";
  let index = 0;
  while (index < text.length) {
    if (text[index] === "[") {
      const end = text.indexOf("]", index + 1);
      if (end !== -1 && isMarker(text.slice(index, end + 1))) {
        index = end + 1;
        continue;
      }
    }
    out += text[index];
    index += 1;
  }
  return out
    .replaceAll("\u00A4", " ")
    .split(/[\n\r]+/)
    .map((line) => line.split(/\s+/).filter(Boolean).join(" "))
    .filter(Boolean)
    .join(" ");
}

/**
 * The add-on whose own settings this row opens through `openSettings`. Rows
 * that open another add-on's settings come marked by the server, which knows
 * the installed add-ons.
 */
export function settingsAddon(file: string): string | null {
  const action = pluginQueryAction(file);
  if (action && sameAscii(action, "openSettings")) return addonId(pluginHost(file));
  return null;
}

function isMarker(token: string): boolean {
  return /^\[\/?[A-Za-z][A-Za-z0-9]*(?:[ =][^\]]*)?\]$/.test(token);
}

function sameAscii(left: string, right: string): boolean {
  if (left.length !== right.length) return false;
  for (let index = 0; index < left.length; index += 1) {
    if (foldAscii(left.charCodeAt(index)) !== foldAscii(right.charCodeAt(index))) return false;
  }
  return true;
}

function foldAscii(code: number): number {
  return code >= 65 && code <= 90 ? code + 32 : code;
}

function pluginAt(file: string): number {
  const needle = "plugin://";
  for (let index = 0; index + needle.length <= file.length; index += 1) {
    let match = true;
    for (let offset = 0; offset < needle.length; offset += 1) {
      if (foldAscii(file.charCodeAt(index + offset)) !== needle.charCodeAt(offset)) {
        match = false;
        break;
      }
    }
    if (match) return index;
  }
  return -1;
}

function stopBuiltin(ch: string): boolean {
  return ch === ")" || ch === "'" || ch === '"';
}

function pluginHost(file: string): string {
  const at = pluginAt(file);
  if (at < 0) return "";
  const rest = file.slice(at + "plugin://".length);
  let host = "";
  for (const ch of rest) {
    if (ch === "/" || ch === "?" || stopBuiltin(ch)) break;
    host += ch;
  }
  return host;
}

function pluginQueryAction(file: string): string | null {
  const at = pluginAt(file);
  if (at < 0) return null;
  const span = file.slice(at);
  let queryAt = -1;
  for (let index = 0; index < span.length; index += 1) {
    const ch = span[index] ?? "";
    if (stopBuiltin(ch)) break;
    if (ch === "?") {
      queryAt = index + 1;
      break;
    }
  }
  if (queryAt < 0) return null;
  let end = span.length;
  for (let index = queryAt; index < span.length; index += 1) {
    if (stopBuiltin(span[index] ?? "")) {
      end = index;
      break;
    }
  }
  for (const pair of span.slice(queryAt, end).split("&")) {
    const eq = pair.indexOf("=");
    if (eq < 0) continue;
    if (sameAscii(pair.slice(0, eq), "action")) return pair.slice(eq + 1).trim();
  }
  return null;
}

function addonId(id: string): string | null {
  if (!id || id.length > 80 || id.includes("..")) return null;
  if (!/^[A-Za-z0-9._-]+$/.test(id)) return null;
  return id;
}
