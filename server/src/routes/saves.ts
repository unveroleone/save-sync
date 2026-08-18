import { FastifyInstance, FastifyRequest, FastifyReply } from 'fastify';
import { requireAuth, getUserName } from '../middleware/auth.js';
import {
  titleDir,
  ensureDir,
  readManifest,
  writeManifest,
  sanitizeTitle,
  rawSavesDir,
  rawGameDir,
  GameEntry,
} from '../storage/disk.js';
import {
  extractMirror,
  canonicalHashOfDir,
  canonicalHashOfZip,
  rebuildZipFromDir,
  sha256OfFile,
  findSyncthingTempFile,
  hasBackslashNames,
} from '../storage/raw.js';
import { randomUUID } from 'crypto';
import fs from 'fs';
import path from 'path';

interface SaveParams {
  titleId: string;
}

/// Decode the X-Save-Title header into a usable title, or undefined.
function decodeTitleHeader(value: unknown): string | undefined {
  if (typeof value !== 'string' || value.length === 0) return undefined;
  try {
    const title = Buffer.from(value, 'base64').toString('utf8');
    return title.length > 0 ? title : undefined;
  } catch {
    return undefined;
  }
}

/// Serializes per-title operations that mutate current.zip + manifest +
/// mirror. Single-process server, so an in-memory promise chain suffices;
/// PUT's critical section and GET's syncRawMirror both go through it.
const titleLocks = new Map<string, Promise<void>>();

async function withTitleLock<T>(titleId: string, fn: () => Promise<T>): Promise<T> {
  const prev = titleLocks.get(titleId) ?? Promise.resolve();
  let release: () => void = () => {};
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  titleLocks.set(titleId, prev.then(() => gate));
  await prev;
  try {
    return await fn();
  } finally {
    release();
  }
}

/// Bring the raw mirror and current.zip back in agreement: extract the
/// mirror when missing, and when it has drifted (edited via Syncthing)
/// rebuild the archive from it, rotating the previous archive into
/// versions/ first. Re-checks after building so a concurrent upload is
/// never overwritten by a stale rebuild (retries once).
async function syncRawMirror(
  app: FastifyInstance,
  userName: string,
  titleId: string,
  currentPath: string,
  rawDir: string
): Promise<void> {
  if (!fs.existsSync(rawDir)) {
    await extractMirror(currentPath, rawDir);
  }
  // Syncthing is mid-transfer: hashing now would bake its temp files into a
  // save. Serve the stored archive and pick the edits up on a later GET.
  // Stale leftovers are skipped too (they must never enter a zip) but get
  // a louder log so the user notices them piling up.
  const syncthingTemp = findSyncthingTempFile(rawDir);
  if (syncthingTemp) {
    if (syncthingTemp.mtimeMs > Date.now() - 10 * 60 * 1000) {
      app.log.debug({ titleId }, 'raw mirror has Syncthing temp files; skipping rebuild this cycle');
    } else {
      app.log.warn(
        { titleId, file: syncthingTemp.rel },
        'stale Syncthing temp file in raw mirror; skipping rebuild'
      );
    }
    return;
  }
  // A file name containing a backslash can never round-trip through yazl
  // (it rewrites \ to /), so the mirror would rebuild on every GET forever.
  if (hasBackslashNames(rawDir)) {
    app.log.warn({ titleId }, 'raw mirror contains file names with backslashes; skipping rebuild');
    return;
  }
  // Remove stale rebuild temp archives from crashed runs.
  const parent = path.dirname(currentPath);
  if (fs.existsSync(parent)) {
    const base = path.basename(currentPath);
    for (const name of fs.readdirSync(parent)) {
      if (name.startsWith(base + '.rebuild-')) {
        fs.rmSync(path.join(parent, name), { force: true });
      }
    }
  }
  for (let attempt = 0; attempt < 2; attempt++) {
    const dirHash = canonicalHashOfDir(rawDir);
    const zipHash = await canonicalHashOfZip(currentPath);
    if (dirHash === zipHash) return;
    const tmpPath = currentPath + '.rebuild-' + randomUUID() + '.tmp';
    await rebuildZipFromDir(rawDir, tmpPath);
    if ((await canonicalHashOfZip(currentPath)) !== zipHash) {
      // Concurrent upload replaced the archive mid-build; discard and retry
      // against the fresh state.
      fs.rmSync(tmpPath, { force: true });
      continue;
    }
    const now = new Date().toISOString();
    if (fs.existsSync(currentPath)) {
      ensureDir(path.join(parent, 'versions'));
      const versionName = now.replace(/[:.]/g, '-') + '.zip';
      fs.renameSync(currentPath, path.join(parent, 'versions', versionName));
    }
    fs.renameSync(tmpPath, currentPath);
    const manifest = readManifest(userName);
    const entry = manifest.games[titleId];
    if (entry) {
      // Never let a server clock move latestVersion backwards; clients
      // treat an older timestamp as "their copy is newer".
      entry.latestVersion =
        entry.latestVersion && entry.latestVersion > now ? entry.latestVersion : now;
      entry.latestHash = sha256OfFile(currentPath);
      entry.size = fs.statSync(currentPath).size;
      // Equals what a client would send for these contents.
      entry.contentHash = dirHash;
      entry.uploadedBy = 'server-raw';
      manifest.updatedAt = new Date().toISOString();
      writeManifest(userName, manifest);
    }
    return;
  }
  app.log.warn({ titleId }, 'raw mirror rebuild aborted after retries; serving stored archive');
}

export async function savesRoutes(app: FastifyInstance): Promise<void> {
  app.put<{ Params: SaveParams }>(
    '/api/save/:titleId',
    { preHandler: requireAuth },
    async (request: FastifyRequest<{ Params: SaveParams }>, reply: FastifyReply) => {
      const { titleId } = request.params;
      const deviceId = (request.headers['x-device-id'] as string) ?? 'unknown';
      const clientHash = request.headers['x-save-hash'] as string | undefined;
      const timestamp =
        (request.headers['x-save-timestamp'] as string) ??
        new Date().toISOString();
      // Game name, base64-encoded UTF-8 (HTTP headers are ASCII only).
      const title = decodeTitleHeader(request.headers['x-save-title']);
      // Canonical content hash of the save files ("sha256:..."), sent by
      // newer clients so devices can compare saves without re-downloading.
      const contentHashHeader = request.headers['x-content-hash'];
      const contentHash =
        typeof contentHashHeader === 'string' &&
        contentHashHeader.length > 0 &&
        contentHashHeader.length <= 128
          ? contentHashHeader
          : undefined;

      const userName = getUserName();
      // The manifest remembers the resolved folder name, so look-alike
      // titleIds can never route an upload into the wrong folder.
      const knownDir = readManifest(userName).games[titleId]?.dir;
      const dir = titleDir(userName, titleId, knownDir);
      ensureDir(dir);

      const tmpPath = path.join(dir, `upload_${randomUUID()}.tmp`);
      const currentPath = path.join(dir, 'current.zip');
      const versionsDir = path.join(dir, 'versions');

      const body = await request.file();
      if (!body) {
        return reply.code(400).send({ ok: false, error: 'No file in request' });
      }

      // stream to temp file via toBuffer (more reliable with Fastify multipart)
      const buffer = await body.toBuffer();
      fs.writeFileSync(tmpPath, buffer);

      // verify hash if provided
      const actualHash = sha256OfFile(tmpPath);
      if (clientHash && clientHash !== actualHash) {
        fs.unlinkSync(tmpPath);
        return reply
          .code(400)
          .send({ ok: false, error: `Hash mismatch: expected ${clientHash}, got ${actualHash}` });
      }

      // rotate current → versions/, install the new zip, update the manifest
      // and mirror under the title lock so a concurrent GET can never
      // rebuild from a stale mirror mid-upload.
      await withTitleLock(titleId, async () => {
        if (fs.existsSync(currentPath)) {
          ensureDir(versionsDir);
          const versionName = timestamp.replace(/[:.]/g, '-') + '.zip';
          fs.renameSync(currentPath, path.join(versionsDir, versionName));
        }

        fs.renameSync(tmpPath, currentPath);
        const size = fs.statSync(currentPath).size;

        // Move the plain-id folder to "<titleId> - <title>" so backups on disk
        // are identifiable, then remember the folder name in the manifest so
        // resolution never has to guess again. A concurrent upload may already
        // have done the rename; losing that race is harmless.
        let resolvedDir = path.basename(dir);
        const sanitizedTitle = title ? sanitizeTitle(title) : '';
        if (sanitizedTitle.length > 0 && resolvedDir === titleId) {
          try {
            const renamed = path.join(path.dirname(dir), `${titleId} - ${sanitizedTitle}`);
            fs.renameSync(dir, renamed);
            resolvedDir = path.basename(renamed);
          } catch (err) {
            // ENOENT: another upload renamed it first. Anything else leaves the
            // folder under its old name, which still resolves via titleDir.
            if ((err as NodeJS.ErrnoException).code !== 'ENOENT') {
              throw err;
            }
          }
        }

        // update manifest
        const manifest = readManifest(userName);
        const entry: GameEntry = {
          ...(manifest.games[titleId] ?? {}),
          latestVersion: timestamp,
          latestHash: actualHash,
          uploadedBy: deviceId,
          size,
        };
        if (sanitizedTitle.length > 0) {
          entry.title = sanitizedTitle;
          entry.dir = resolvedDir;
        }
        if (contentHash) {
          entry.contentHash = contentHash;
        }
        manifest.games[titleId] = entry;
        manifest.updatedAt = new Date().toISOString();
        writeManifest(userName, manifest);

        const rawRoot = rawSavesDir();
        if (rawRoot) {
          try {
            const rawDir = rawGameDir(resolvedDir);
            // `dir`/`currentPath` still point at the pre-rename location;
            // rebuild the canonical path from resolvedDir.
            const currentZip = path.join(path.dirname(dir), resolvedDir, 'current.zip');
            await extractMirror(currentZip, rawDir);
            if (!contentHash) {
              // Older clients send no content hash; the mirror's canonical
              // hash is exactly the value they would have sent. Re-read the
              // manifest: the awaited extraction above can interleave with
              // other manifest writes, and patching a stale snapshot would
              // silently drop them.
              const fresh = readManifest(userName);
              const freshEntry = fresh.games[titleId];
              if (freshEntry) {
                freshEntry.contentHash = canonicalHashOfDir(rawDir);
                fresh.updatedAt = new Date().toISOString();
                writeManifest(userName, fresh);
              }
            }
          } catch (err) {
            app.log.warn({ err, titleId }, 'raw mirror extraction failed; upload kept as-is');
            try {
              // A stale mirror would make the next GET "see" old files as
              // drift and roll the fresh upload back. Remove it so the
              // next GET re-extracts from the uploaded zip instead.
              fs.rmSync(rawGameDir(resolvedDir), { recursive: true, force: true });
            } catch (rmErr) {
              app.log.warn({ err: rmErr, titleId }, 'failed to remove stale raw mirror');
            }
          }
        }
      });

      reply.send({ ok: true, titleId, version: timestamp, hash: actualHash });
    }
  );

  app.get<{ Params: SaveParams }>(
    '/api/save/:titleId',
    { preHandler: requireAuth },
    async (request: FastifyRequest<{ Params: SaveParams }>, reply: FastifyReply) => {
      const { titleId } = request.params;
      const userName = getUserName();
      const manifest = readManifest(userName);
      const entry = manifest.games[titleId];
      const currentPath = path.join(titleDir(userName, titleId, entry?.dir), 'current.zip');

      if (!fs.existsSync(currentPath)) {
        return reply.code(404).send({ ok: false, error: 'No save found for ' + titleId });
      }

      // Read the archive under the title lock so a concurrent PUT/DELETE
      // cannot swap current.zip between the manifest read and the bytes we
      // serve (mismatched hash headers would force clients to redownload).
      let servedEntry = entry;
      const rawRoot = rawSavesDir();
      let data: Buffer;
      if (rawRoot) {
        try {
          data = await withTitleLock(titleId, async () => {
            await syncRawMirror(
              app,
              userName,
              titleId,
              currentPath,
              rawGameDir(path.basename(path.dirname(currentPath)))
            );
            servedEntry = readManifest(userName).games[titleId] ?? entry;
            return fs.readFileSync(currentPath);
          });
        } catch (err) {
          app.log.warn({ err, titleId }, 'raw mirror sync failed; serving stored archive');
          data = fs.readFileSync(currentPath);
        }
      } else {
        data = fs.readFileSync(currentPath);
      }

      reply
        .header('Content-Type', 'application/zip')
        .header('Content-Disposition', `attachment; filename="${titleId}.zip"`)
        .header('X-Save-Hash', servedEntry?.latestHash ?? '')
        .header('X-Save-Timestamp', servedEntry?.latestVersion ?? '')
        .send(data);
    }
  );

  app.delete<{ Params: SaveParams }>(
    '/api/save/:titleId',
    { preHandler: requireAuth },
    async (request: FastifyRequest<{ Params: SaveParams }>, reply: FastifyReply) => {
      const { titleId } = request.params;
      const userName = getUserName();
      const knownDir = readManifest(userName).games[titleId]?.dir;
      const dir = titleDir(userName, titleId, knownDir);

      if (!fs.existsSync(dir)) {
        return reply.code(404).send({ ok: false, error: 'No save found for ' + titleId });
      }

      await withTitleLock(titleId, async () => {
        fs.rmSync(dir, { recursive: true, force: true });

        const rawRoot = rawSavesDir();
        if (rawRoot) {
          try {
            fs.rmSync(rawGameDir(path.basename(dir)), { recursive: true, force: true });
          } catch (err) {
            app.log.warn({ err, titleId }, 'failed to remove raw mirror');
          }
        }

        const manifest = readManifest(userName);
        delete manifest.games[titleId];
        manifest.updatedAt = new Date().toISOString();
        writeManifest(userName, manifest);
      });

      reply.send({ ok: true, titleId });
    }
  );
}
