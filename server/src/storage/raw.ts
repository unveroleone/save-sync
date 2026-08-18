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
