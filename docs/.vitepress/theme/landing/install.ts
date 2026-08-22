/**
 * The install one-liners the landing puts above the fold. The full list
 * (apt, dnf, AUR, Nix, manual, from source) lives in
 * docs/getting-started/installation.md — when a primary command there
 * changes, change it here in the same edit; this is the fold, that is the
 * page.
 */

export type Platform = "macos" | "linux" | "windows";

export interface InstallLine {
  id: Platform;
  label: string;
  /** `sh` prompts with `$`, `pwsh` with `>`. */
  shell: "sh" | "pwsh";
  command: string;
}

export const INSTALL: InstallLine[] = [
  {
    id: "macos",
    label: "macOS",
    shell: "sh",
    command: "brew install avihut/tap/daft",
  },
  {
    id: "linux",
    label: "Linux",
    shell: "sh",
    command:
      "curl --proto '=https' --tlsv1.2 -LsSf https://github.com/avihut/daft/releases/latest/download/daft-installer.sh | sh",
  },
  {
    id: "windows",
    label: "Windows",
    shell: "pwsh",
    command:
      "irm https://github.com/avihut/daft/releases/latest/download/daft-installer.ps1 | iex",
  },
];

export const INSTALL_DOCS = "/getting-started/installation";
export const QUICK_START = "/getting-started/quick-start";

/** Best-effort platform guess for the default tab; the tabs stay clickable. */
export function detectPlatform(userAgent: string, platform: string): Platform {
  const s = `${platform} ${userAgent}`;
  if (/mac|iphone|ipad|darwin/i.test(s)) return "macos";
  if (/win/i.test(s)) return "windows";
  return "linux";
}
