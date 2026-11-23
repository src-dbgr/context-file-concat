/**
 * What the UI has to know about the platform it runs on. One place, so that
 * nothing else reads the user agent.
 */

/** What of `navigator` tells the platform. */
export type PlatformHints = {
  /** Chromium (WebView2); not in WebKit. */
  userAgentData?: { platform?: string };
  platform?: string;
  userAgent?: string;
};

/**
 * Whether the keyboard is a Mac's. There the key labelled "delete" is
 * Backspace (`⌫`); forward delete (`KeyboardEvent.key === "Delete"`) is
 * `fn+⌫`, or `⌦` on a full keyboard.
 */
export function isMac(
  hints: PlatformHints | undefined = typeof navigator === "undefined"
    ? undefined
    : (navigator as PlatformHints),
): boolean {
  if (!hints) return false;
  const platform = hints.userAgentData?.platform || hints.platform || "";
  if (platform) return /^mac/i.test(platform);
  return /Macintosh|Mac OS X/.test(hints.userAgent ?? "");
}
