// Numbers and times for people.

export function ago(seconds: number | null | undefined): string {
  if (!seconds) return "never";
  const d = Date.now() / 1000 - seconds;
  if (d < 45) return "just now";
  if (d < 90) return "a minute ago";
  if (d < 3600) return `${Math.round(d / 60)} minutes ago`;
  if (d < 5400) return "an hour ago";
  if (d < 86400) return `${Math.round(d / 3600)} hours ago`;
  if (d < 172800) return "yesterday";
  return new Date(seconds * 1000).toLocaleDateString();
}

export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

export function count(n: number, one: string, many = `${one}s`): string {
  return `${n.toLocaleString()} ${n === 1 ? one : many}`;
}

/** The last part of a path. */
export function base(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

/** The folder of a path. */
export function parent(path: string): string {
  const parts = path.split(/[\\/]/);
  parts.pop();
  return parts.join("/");
}
