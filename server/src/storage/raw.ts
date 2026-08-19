import fs from 'fs';
import path from 'path';
import crypto, { randomUUID } from 'crypto';
import yauzl from 'yauzl';
import yazl from 'yazl';

export interface NamedFile {
  name: string;
  bytes: Buffer;
}

/// Byte-wise UTF-8 compare, matching Rust `String::cmp` in content_hash.
function byName(a: NamedFile, b: NamedFile): number {
  return Buffer.from(a.name, 'utf8').compare(Buffer.from(b.name, 'utf8'));
}

/// Core of the canonical hash, shared by the directory walk and the zip
/// reader so the two can never drift apart. Exact port of
/// save-sync-hub/src-tauri/src/backup.rs content_hash: one sha256 over
/// `name`, 0x00, `bytes` for every file, concatenated in sorted order
/// (not per-file hashes). Directory entries are never hashed.
export function canonicalHash(files: NamedFile[]): string {
  const sorted = [...files].sort(byName);
  const h = crypto.createHash('sha256');
  for (const f of sorted) {
    h.update(f.name, 'utf8');
    h.update(Buffer.from([0x00]));
    h.update(f.bytes);
  }
  return 'sha256:' + h.digest('hex');
}

/// All files below `dir` as ('/'-separated rel name, bytes). Port of
/// collect_rel_files + content_hash: ALL files included (no hidden filter),
/// dirs recursed but not hashed, sorted only at hash time. statSync follows
/// symlinks like Rust Path::is_dir/is_file (do NOT use Dirent.isFile()).
export function collectFilesOfDir(dir: string, prefix = ''): NamedFile[] {
  const out: NamedFile[] = [];
  for (const name of fs.readdirSync(dir)) {
    const full = path.join(dir, name);
    const rel = prefix === '' ? name : `${prefix}/${name}`;
    const st = fs.statSync(full);
    if (st.isDirectory()) {
      out.push(...collectFilesOfDir(full, rel));
    } else if (st.isFile()) {
      out.push({ name: rel, bytes: fs.readFileSync(full) });
    }
    // sockets/fifos ignored; save trees never contain them.
  }
  return out;
}

export function canonicalHashOfDir(dir: string): string {
  return canonicalHash(collectFilesOfDir(dir));
}

/// Names-only variant of collectFilesOfDir, sorted byte-wise. Used by the
/// zip builder, which only needs the paths (yazl stats the files itself).
export function collectFileNamesOfDir(dir: string, prefix = ''): string[] {
  const out: string[] = [];
  for (const name of fs.readdirSync(dir)) {
    const full = path.join(dir, name);
    const rel = prefix === '' ? name : `${prefix}/${name}`;
    const st = fs.statSync(full);
    if (st.isDirectory()) {
      out.push(...collectFileNamesOfDir(full, rel));
    } else if (st.isFile()) {
      out.push(rel);
    }
  }
  return out.sort((a, b) => Buffer.from(a, 'utf8').compare(Buffer.from(b, 'utf8')));
}

/// Returns the rel path and mtime of a Syncthing temp file
/// (`.syncthing.<name>.<rand>.tmp`) if the tree contains one, else null.
/// Freshness is the caller's call; this reports every match so stale
/// leftovers can be logged instead of silently baked into a rebuild.
export function findSyncthingTempFile(
  dir: string,
  prefix = ''
): { rel: string; mtimeMs: number } | null {
  for (const name of fs.readdirSync(dir)) {
    const full = path.join(dir, name);
    const rel = prefix === '' ? name : `${prefix}/${name}`;
    const st = fs.statSync(full);
    if (st.isDirectory()) {
      const found = findSyncthingTempFile(full, rel);
      if (found) return found;
    } else if (st.isFile() && name.startsWith('.syncthing.')) {
      return { rel, mtimeMs: st.mtimeMs };
    }
  }
  return null;
}

/// True when any file name in the tree contains a backslash. yazl rewrites
/// `\` to `/` in metadata paths, so such a file could never round-trip: the
/// mirror hash would diverge from the zip hash forever and every GET would
/// rebuild. Real Vita saves (FAT filesystem) cannot contain these names.
export function hasBackslashNames(dir: string, prefix = ''): boolean {
  for (const name of fs.readdirSync(dir)) {
    if (name.includes('\\')) return true;
    const full = path.join(dir, name);
    const rel = prefix === '' ? name : `${prefix}/${name}`;
    if (fs.statSync(full).isDirectory() && hasBackslashNames(full, rel)) return true;
  }
  return false;
}

export function sha256OfFile(filePath: string): string {
  const h = crypto.createHash('sha256');
  h.update(fs.readFileSync(filePath));
  return 'sha256:' + h.digest('hex');
}

function openZip(zipPath: string): Promise<yauzl.ZipFile> {
  return new Promise((resolve, reject) => {
    yauzl.open(zipPath, { lazyEntries: true, autoClose: true }, (err, zipfile) => {
      if (err || !zipfile) {
        reject(err ?? new Error('open zip failed: ' + zipPath));
      } else {
        resolve(zipfile);
      }
    });
  });
}

function readEntryBytes(zipfile: yauzl.ZipFile, entry: yauzl.Entry): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    zipfile.openReadStream(entry, (err, stream) => {
      if (err || !stream) {
        reject(err ?? new Error('read entry failed: ' + entry.fileName));
        return;
      }
      const chunks: Buffer[] = [];
      stream.on('data', (c: Buffer) => chunks.push(c));
      stream.on('end', () => resolve(Buffer.concat(chunks)));
      stream.on('error', reject);
    });
  });
}

/// Canonical hash of a zip's contents: same algorithm over file entries,
/// directory entries (names ending '/') skipped. yauzl already rewrites
/// '\' to '/' and validates names that would escape the archive.
export function canonicalHashOfZip(zipPath: string): Promise<string> {
  return openZip(zipPath).then(
    (zipfile) =>
      new Promise<string>((resolve, reject) => {
        const files: NamedFile[] = [];
        const fail = (err: Error) => {
          // autoClose only closes on zipfile events; close explicitly so an
          // aborted entry read does not leak the fd.
          try {
            zipfile.close();
          } catch {
            // already closed
          }
          reject(err);
        };
        zipfile.on('error', fail);
        zipfile.on('entry', (entry: yauzl.Entry) => {
          if (entry.fileName.endsWith('/')) {
            zipfile.readEntry();
            return;
          }
          readEntryBytes(zipfile, entry)
            .then((bytes) => {
              files.push({ name: entry.fileName, bytes });
              zipfile.readEntry();
            })
            .catch(fail);
        });
        zipfile.on('end', () => resolve(canonicalHash(files)));
        zipfile.readEntry();
      })
  );
}

/// Belt-and-braces zip-slip check. yauzl already validates entry names
/// (absolute paths, drive letters, `..` segments) before emitting the
/// entry event, so this is defense-in-depth only and normally never fires.
function unsafeEntryName(name: string): boolean {
  if (name.startsWith('/')) return true;
  if (/^[a-zA-Z]:[\\/]/.test(name)) return true;
  return name.split('/').includes('..');
}

/// Remove stale temp/backup siblings of destDir left behind by a crashed
/// extraction or rebuild attempt.
function cleanStaleSiblings(destDir: string, patterns: string[]): void {
  const parent = path.dirname(destDir);
  if (!fs.existsSync(parent)) return;
  const base = path.basename(destDir);
  for (const name of fs.readdirSync(parent)) {
    if (patterns.some((p) => name.startsWith(base + p))) {
      fs.rmSync(path.join(parent, name), { recursive: true, force: true });
    }
  }
}

/// Extract a zip into destDir atomically: into a temp sibling, old mirror
/// moved aside, temp renamed in, old removed. On any failure the temp is
/// cleaned up and the previous mirror (if any) restored/untouched.
export function extractMirror(zipPath: string, destDir: string): Promise<void> {
  cleanStaleSiblings(destDir, ['.tmp-', '.old-']);
  const tmpDir = destDir + '.tmp-' + randomUUID();
  const backupDir = destDir + '.old-' + randomUUID();
  fs.mkdirSync(tmpDir, { recursive: true });
  return openZip(zipPath)
    .then(
      (zipfile) =>
        new Promise<void>((resolve, reject) => {
          let failed = false;
          const fail = (err: Error) => {
            failed = true;
            // autoClose only closes on zipfile events; close explicitly so
            // an aborted entry read does not leak the fd.
            try {
              zipfile.close();
            } catch {
              // already closed
            }
            reject(err);
          };
          zipfile.on('error', fail);
          zipfile.on('entry', (entry: yauzl.Entry) => {
            if (failed) return;
            const name = entry.fileName;
            if (unsafeEntryName(name)) {
              fail(new Error('unsafe zip entry: ' + name));
              return;
            }
            if (name.endsWith('/')) {
              fs.mkdirSync(path.join(tmpDir, name), { recursive: true });
              zipfile.readEntry();
              return;
            }
            const outPath = path.join(tmpDir, name);
            fs.mkdirSync(path.dirname(outPath), { recursive: true });
            readEntryBytes(zipfile, entry)
              .then((bytes) => {
                fs.writeFileSync(outPath, bytes);
                zipfile.readEntry();
              })
              .catch(fail);
          });
          zipfile.on('end', () => {
            try {
              if (fs.existsSync(destDir)) fs.renameSync(destDir, backupDir);
              fs.renameSync(tmpDir, destDir);
              // The swap succeeded; a leftover backup is only cleaned up
              // best-effort (a locked file must not fail the upload).
              try {
                fs.rmSync(backupDir, { recursive: true, force: true });
              } catch {
                // stale .old- sibling is removed by the next run
              }
              resolve();
            } catch (err) {
              if (fs.existsSync(backupDir) && !fs.existsSync(destDir)) {
                fs.renameSync(backupDir, destDir);
              }
              reject(err);
            }
          });
          zipfile.readEntry();
        })
    )
    .catch((err) => {
      fs.rmSync(tmpDir, { recursive: true, force: true });
      throw err;
    });
}

/// Write a zip holding every file below `dir` under its '/'-separated rel
/// name. No directory entries — they are not needed and are never hashed.
/// Temp file + rename, so readers never see a half-written archive. Note
/// the zip bytes are not fully deterministic (yazl embeds file mtimes);
/// the canonical content hash ignores zip metadata, so this is fine.
export function rebuildZipFromDir(dir: string, zipPath: string): Promise<void> {
  const rels = collectFileNamesOfDir(dir);
  const tmpPath = zipPath + '.tmp-' + randomUUID();
  const zip = new yazl.ZipFile();
  return new Promise<void>((resolve, reject) => {
    let failed = false;
    const out = fs.createWriteStream(tmpPath);
    zip.on('error', (err: Error) => {
      failed = true;
      out.destroy();
      try {
        fs.rmSync(tmpPath, { force: true });
      } catch {
        // tmp already gone
      }
      reject(err);
    });
    zip.outputStream.pipe(out);
    out.on('close', () => {
      // 'close' also fires after 'error'/destroy(); never rename a
      // half-written archive into place.
      if (failed) return;
      try {
        fs.renameSync(tmpPath, zipPath);
        resolve();
      } catch (err) {
        try {
          fs.rmSync(tmpPath, { force: true });
        } catch {
          // tmp already gone
        }
        reject(err);
      }
    });
    out.on('error', (err) => {
      failed = true;
      try {
        fs.rmSync(tmpPath, { force: true });
      } catch {
        // tmp already gone
      }
      reject(err);
    });
    for (const rel of rels) {
      zip.addFile(path.join(dir, rel), rel);
    }
    zip.end();
  });
}

// --- Flat/shared-root mirror mode ---------------------------------------
//
// The functions above assume one raw directory belongs to exactly one
// title. RetroArch saves break that assumption: each ROM is its own title
// (so sync/versioning stays per-game), but users expect the *mirror* to
// look like RetroArch's own layout — files grouped by core, not nested
// under a wrapper folder per game (see GitHub issue #6 follow-up). RetroArch
// entry names already encode "savefiles/<core>/<rom>.srm" and are unique
// per ROM filename, so multiple titles can safely share one root directory
// as long as every operation is scoped to a specific title's own known
// paths instead of walking the whole tree.
//
// "Known paths" for a title are always re-derived by opening its current
// zip (zipEntryNames) rather than stored anywhere — the zip is already the
// source of truth for what that title owns.

/// True for titles whose raw mirror should use the flat/shared-root layout
/// instead of a per-title wrapper folder. Scoped narrowly to RetroArch: its
/// entry names are ROM-filename-unique, so collisions across titles sharing
/// the root are not realistic. Other save sources (PSP, native Vita saves)
/// can use generic file names and are NOT safe to flatten this way.
export function isFlatMirrorTitle(titleId: string): boolean {
  return titleId.startsWith('RETROARCH_');
}

/// Relative paths of a zip's file entries (directory entries skipped), in
/// listing order.
export function zipEntryNames(zipPath: string): Promise<string[]> {
  return openZip(zipPath).then(
    (zipfile) =>
      new Promise<string[]>((resolve, reject) => {
        const names: string[] = [];
        const fail = (err: Error) => {
          try {
            zipfile.close();
          } catch {
            // already closed
          }
          reject(err);
        };
        zipfile.on('error', fail);
        zipfile.on('entry', (entry: yauzl.Entry) => {
          if (!entry.fileName.endsWith('/')) {
            names.push(entry.fileName);
          }
          zipfile.readEntry();
        });
        zipfile.on('end', () => resolve(names));
        zipfile.readEntry();
      })
  );
}

/// Canonical hash over an explicit set of relative paths read from baseDir.
/// A path that no longer exists is simply excluded — rebuildZipFromPaths
/// treats a missing path the same way, so both sides of a drift comparison
/// agree on what "the current content" is when the user deleted a file.
export function canonicalHashOfPaths(baseDir: string, relPaths: string[]): string {
  const files: NamedFile[] = [];
  for (const rel of relPaths) {
    const full = path.join(baseDir, rel);
    if (fs.existsSync(full) && fs.statSync(full).isFile()) {
      files.push({ name: rel, bytes: fs.readFileSync(full) });
    }
  }
  return canonicalHash(files);
}

/// Extract a zip's entries directly under baseDir at their own path — no
/// per-title wrapper folder, since baseDir is shared with other titles.
/// Each file is written via temp+rename (no whole-tree swap, unlike
/// extractMirror: a directory swap here would delete unrelated titles'
/// files that happen to live under the same subfolders).
export function extractMirrorFlat(zipPath: string, baseDir: string): Promise<void> {
  return openZip(zipPath).then(
    (zipfile) =>
      new Promise<void>((resolve, reject) => {
        let failed = false;
        const fail = (err: Error) => {
          failed = true;
          try {
            zipfile.close();
          } catch {
            // already closed
          }
          reject(err);
        };
        zipfile.on('error', fail);
        zipfile.on('entry', (entry: yauzl.Entry) => {
          if (failed) return;
          const name = entry.fileName;
          if (name.endsWith('/')) {
            zipfile.readEntry();
            return;
          }
          if (unsafeEntryName(name)) {
            fail(new Error('unsafe zip entry: ' + name));
            return;
          }
          const outPath = path.join(baseDir, name);
          readEntryBytes(zipfile, entry)
            .then((bytes) => {
              fs.mkdirSync(path.dirname(outPath), { recursive: true });
              const tmp = outPath + '.tmp-' + randomUUID();
              fs.writeFileSync(tmp, bytes);
              fs.renameSync(tmp, outPath);
              zipfile.readEntry();
            })
            .catch(fail);
        });
        zipfile.on('end', () => resolve());
        zipfile.readEntry();
      })
  );
}

/// Write a zip from an explicit set of relative paths read from baseDir.
/// Paths that no longer exist are dropped from the archive — the same
/// "missing = deleted" semantics as canonicalHashOfPaths.
export function rebuildZipFromPaths(
  baseDir: string,
  relPaths: string[],
  zipPath: string
): Promise<void> {
  const rels = [...relPaths]
    .filter((rel) => fs.existsSync(path.join(baseDir, rel)))
    .sort((a, b) => Buffer.from(a, 'utf8').compare(Buffer.from(b, 'utf8')));
  const tmpPath = zipPath + '.tmp-' + randomUUID();
  const zip = new yazl.ZipFile();
  return new Promise<void>((resolve, reject) => {
    let failed = false;
    const out = fs.createWriteStream(tmpPath);
    zip.on('error', (err: Error) => {
      failed = true;
      out.destroy();
      try {
        fs.rmSync(tmpPath, { force: true });
      } catch {
        // tmp already gone
      }
      reject(err);
    });
    zip.outputStream.pipe(out);
    out.on('close', () => {
      if (failed) return;
      try {
        fs.renameSync(tmpPath, zipPath);
        resolve();
      } catch (err) {
        try {
          fs.rmSync(tmpPath, { force: true });
        } catch {
          // tmp already gone
        }
        reject(err);
      }
    });
    out.on('error', (err) => {
      failed = true;
      try {
        fs.rmSync(tmpPath, { force: true });
      } catch {
        // tmp already gone
      }
      reject(err);
    });
    for (const rel of rels) {
      zip.addFile(path.join(baseDir, rel), rel);
    }
    zip.end();
  });
}

/// Scoped Syncthing-temp-file check for the flat mirror: only looks in the
/// parent directories of this title's own known paths, so one title's
/// in-flight transfer never blocks unrelated titles sharing the root.
export function findSyncthingTempFileForPaths(
  baseDir: string,
  relPaths: string[]
): { rel: string; mtimeMs: number } | null {
  const parents = new Set(relPaths.map((r) => path.dirname(r)));
  for (const parentRel of parents) {
    const parentAbs = path.join(baseDir, parentRel);
    if (!fs.existsSync(parentAbs)) continue;
    for (const name of fs.readdirSync(parentAbs)) {
      if (!name.startsWith('.syncthing.')) continue;
      const full = path.join(parentAbs, name);
      if (fs.statSync(full).isFile()) {
        return {
          rel: parentRel === '.' ? name : `${parentRel}/${name}`,
          mtimeMs: fs.statSync(full).mtimeMs,
        };
      }
    }
  }
  return null;
}

/// The RetroArch title's ROM-name stem ("RETROARCH_<stem>" -> "<stem>"), or
/// null if titleId is not a flat-mirror title.
export function retroarchStemFromTitleId(titleId: string): string | null {
  const prefix = 'RETROARCH_';
  return titleId.startsWith(prefix) ? titleId.slice(prefix.length) : null;
}

/// True when a zip entry's basename stem (the part before its last dot,
/// same split rule the Vita client uses to group save files by ROM) matches
/// the given RetroArch stem. The client is trusted to only ever put a
/// title's own files in its zip, but the flat mirror shares its root
/// directory across titles, so the server verifies this before writing or
/// deleting anything there — a malformed upload must not touch another
/// title's mirrored files.
export function entryMatchesRetroarchStem(entryName: string, stem: string): boolean {
  const base = entryName.split('/').pop() ?? entryName;
  const dot = base.lastIndexOf('.');
  const entryStem = dot === -1 ? base : base.slice(0, dot);
  return entryStem === stem;
}

/// Remove leftover `<path>.tmp-*` siblings for any of the given relPaths,
/// left behind by a crashed extractMirrorFlat write.
export function cleanStaleFlatTemps(baseDir: string, relPaths: string[]): void {
  const parents = new Set(relPaths.map((r) => path.dirname(r)));
  for (const parentRel of parents) {
    const parentAbs = path.join(baseDir, parentRel);
    if (!fs.existsSync(parentAbs)) continue;
    for (const name of fs.readdirSync(parentAbs)) {
      if (relPaths.some((r) => name.startsWith(path.basename(r) + '.tmp-'))) {
        fs.rmSync(path.join(parentAbs, name), { force: true });
      }
    }
  }
}
