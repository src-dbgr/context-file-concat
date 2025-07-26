// Reine Hilfsfunktionen ohne Seiteneffekte.
export function formatFileSize(bytes) {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

function countWords(text) {
  if (!text || text.trim() === "") return 0;
  const words = text.trim().split(/\s+/);
  return words.filter((word) => word.length > 0).length;
}

function countCharacters(text) {
  if (!text) return 0;
  return text.length;
}

function formatNumber(num) {
  if (num >= 1000000) return (num / 1000000).toFixed(1) + "M";
  if (num >= 1000) return (num / 1000).toFixed(1) + "K";
  return num.toString();
}

export function generateStatsString(content, additionalInfo = "") {
  const lines = content.split("\n").length;
  const words = countWords(content);
  const characters = countCharacters(content);
  const sizeBytes = new Blob([content], { type: "text/plain" }).size;
  const sizeFormatted = formatFileSize(sizeBytes);
  const formattedWords = formatNumber(words);
  const formattedChars = formatNumber(characters);
  let statsString = `${lines} lines • ${formattedWords} words • ${formattedChars} chars • ${sizeFormatted}`;
  if (additionalInfo) statsString += ` • ${additionalInfo}`;
  return statsString;
}

export function splitPathForDisplay(fullPath, currentDir) {
    if (!fullPath) return { pathPart: "", filename: "Unknown File" };
    let relativePath = fullPath;

    if (currentDir && fullPath.startsWith(currentDir)) {
      relativePath = fullPath.substring(currentDir.length);
      relativePath = relativePath.replace(/^[\/\\]+/, "");
    }

    const parts = relativePath.replace(/\\/g, "/").split("/");
    if (parts.length <= 1) {
      return { pathPart: "", filename: relativePath };
    }

    const filename = parts[parts.length - 1];
    const pathPart = parts.slice(0, -1).join("/") + "/";
    return { pathPart, filename };
}
