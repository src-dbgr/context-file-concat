export function formatFileSize(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

function countWords(text: string): number {
  if (!text || text.trim() === "") return 0;
  const words = text.trim().split(/\s+/);
  return words.filter((word) => word.length > 0).length;
}

function countCharacters(text: string): number {
  if (!text) return 0;
  return text.length;
}

function formatNumber(num: number): string {
  if (num === undefined || num === null) return "...";
  if (num >= 1000000) return (num / 1000000).toFixed(1) + "M";
  if (num >= 1000) return (num / 1000).toFixed(1) + "K";
  return num.toString();
}

export function generateStatsString(
  content: string,
  additionalInfo = "",
  tokenCount?: number
): string {
  const lines = content.split("\n").length;
  const words = countWords(content);
  const characters = countCharacters(content);
  const statsParts = [
    `${lines} lines`,
    `${formatNumber(words)} words`,
    `${formatNumber(characters)} chars`,
  ];
  if (typeof tokenCount === "number") {
    statsParts.push(`${formatNumber(tokenCount)} tokens`);
  }
  statsParts.push(formatFileSize(new Blob([content]).size));
  if (additionalInfo) {
    statsParts.push(additionalInfo);
  }
  return statsParts.join(" • ");
}

export function splitPathForDisplay(
  fullPath: string,
  currentDir: string | null
): { pathPart: string; filename: string } {
  if (!fullPath) return { pathPart: "", filename: "Unknown File" };
  let relativePath = fullPath;

  if (currentDir && fullPath.startsWith(currentDir)) {
    relativePath = fullPath.substring(currentDir.length);
    relativePath = relativePath.replace(/^[/\\]+/, "");
  }

  const parts = relativePath.replace(/\\/g, "/").split("/");
  if (parts.length <= 1) {
    return { pathPart: "", filename: relativePath };
  }

  const filename = parts[parts.length - 1];
  const pathPart = parts.slice(0, -1).join("/") + "/";
  return { pathPart, filename };
}
