import fs from 'fs';
import path from 'path';

export function dataDir(): string {
  return process.env.DATA_DIR ?? path.join(process.cwd(), 'data');
}

export function userDir(userName: string): string {
  return path.join(dataDir(), 'users', userName);
}

export function savesDir(userName: string): string {
  return path.join(userDir(userName), 'saves');
}

/// Strip characters that would break a filesystem path (control bytes and
/// path-hostile punctuation; dashes stay, they are legal in filenames).
export function sanitizeTitle(title: string): string {
  const cleaned = title
    // eslint-disable-next-line no-control-regex
    .replace(/[\/\\:*?"<>|\x00-\x1f\x7f]/g, '_')
    .trim()
    .replace(/\s+/g, ' ')
    .slice(0, 80);
  return cleaned;
}

/// Saves uploaded with a title are stored under "<titleId> - <title>". Resolve
/// in order: the folder name remembered in the manifest (authoritative),
/// the exact id (pre-rename folders, older clients), then the labelled folder.
export function titleDir(userName: string, titleId: string, knownDir?: string): string {
  const saves = savesDir(userName);
  if (knownDir && knownDir.length > 0) {
    const known = path.join(saves, knownDir);
    if (fs.existsSync(known)) return known;
  }
  const exact = path.join(saves, titleId);
  if (fs.existsSync(exact)) return exact;
  if (!fs.existsSync(saves)) return exact;
  const prefix = titleId + ' - ';
  const labelled = fs
    .readdirSync(saves)
    .find((name) => name.startsWith(prefix) && name.length > prefix.length);
  return labelled ? path.join(saves, labelled) : exact;
}

export function manifestPath(userName: string): string {
  return path.join(userDir(userName), 'manifest.json');
}

export function devicesDir(userName: string): string {
  return path.join(userDir(userName), 'devices');
}

export function ensureDir(dir: string): void {
  fs.mkdirSync(dir, { recursive: true });
}

export interface GameEntry {
  title?: string;
  /// Folder name under saves/ for this game, remembered after the rename so
  /// resolution never has to guess between look-alike titleIds.
  dir?: string;
  /// Canonical content hash of the save files, sent by newer clients so
  /// devices can compare saves without re-downloading anything.
  contentHash?: string;
  latestVersion: string;
  latestHash: string;
  uploadedBy: string;
  size: number;
  versionCount?: number;
}

export function countVersions(userName: string, titleId: string, knownDir?: string): number {
  const versionsPath = path.join(titleDir(userName, titleId, knownDir), 'versions');
  if (!fs.existsSync(versionsPath)) return 0;
  return fs.readdirSync(versionsPath).filter(f => f.endsWith('.zip')).length;
}

export interface Manifest {
  userId: string;
  updatedAt: string;
  games: Record<string, GameEntry>;
}

export function readManifest(userName: string): Manifest {
  const mp = manifestPath(userName);
  if (!fs.existsSync(mp)) {
    return { userId: userName, updatedAt: new Date().toISOString(), games: {} };
  }
  return JSON.parse(fs.readFileSync(mp, 'utf8')) as Manifest;
}

export function writeManifest(userName: string, manifest: Manifest): void {
  ensureDir(userDir(userName));
  fs.writeFileSync(manifestPath(userName), JSON.stringify(manifest, null, 2));
}
