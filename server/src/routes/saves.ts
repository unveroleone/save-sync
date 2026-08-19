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
  isFlatMirrorTitle,
  zipEntryNames,
  canonicalHashOfPaths,
  extractMirrorFlat,
  rebuildZipFromPaths,
  findSyncthingTempFileForPaths,
  retroarchStemFromTitleId,
  entryMatchesRetroarchStem,
  cleanStaleFlatTemps,
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

/// Rotates the previous archive into versions/, installs tmpPath as the new
/// current.zip, and updates the manifest to match. Shared by both mirror
/// modes' rebuild path.
function installRebuiltZip(
  userName: string,
  titleId: string,
  currentPath: string,
  tmpPath: string,
  contentHash: string
): void {
  const parent = path.dirname(currentPath);
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
    // Never let a server clock move latestVersion backwards; clients treat
    // an older timestamp as "their copy is newer".
    entry.latestVersion =
      entry.latestVersion && entry.latestVersion > now ? entry.latestVersion : now;
    entry.latestHash = sha256OfFile(currentPath);
    entry.size = fs.statSync(currentPath).size;
    // Equals what a client would send for these contents.
    entry.contentHash = contentHash;
    entry.uploadedBy = 'server-raw';
    manifest.updatedAt = new Date().toISOString();
    writeManifest(userName, manifest);
  }
}

/// Removes stale `<currentPath>.rebuild-*.tmp` siblings left by a crashed
/// rebuild attempt.
function cleanStaleRebuildTemps(currentPath: string): void {
  const parent = path.dirname(currentPath);
  if (!fs.existsSync(parent)) return;
  const base = path.basename(currentPath);
  for (const name of fs.readdirSync(parent)) {
    if (name.startsWith(base + '.rebuild-')) {
      fs.rmSync(path.join(parent, name), { force: true });
    }
  }
}

/// Bring the flat/shared-root mirror and current.zip back in agreement for
/// a RetroArch title. Every operation is scoped to this title's own known
/// paths (re-derived from its zip each time), never a directory walk, since
/// mirrorBase is shared with other titles.
async function syncRawMirrorFlat(
  app: FastifyInstance,
  userName: string,
  titleId: string,
  currentPath: string,
  mirrorBase: string
): Promise<void> {
  const initialPaths = await zipEntryNames(currentPath);
  // Directory existence cannot tell "never mirrored" from "mirrored, then
  // the user deleted everything" for a flat title, since mirrorBase is
  // shared with other titles and persists regardless. Use the manifest's
  // explicit flag instead — set once extraction succeeds, here or in PUT.
  const bootstrapManifest = readManifest(userName);
  const bootstrapEntry = bootstrapManifest.games[titleId];
  if (!bootstrapEntry?.rawMirrored && initialPaths.length > 0) {
    await extractMirrorFlat(currentPath, mirrorBase);
    if (bootstrapEntry) {
      bootstrapEntry.rawMirrored = true;
      bootstrapManifest.updatedAt = new Date().toISOString();
      writeManifest(userName, bootstrapManifest);
    }
  }
  const syncthingTemp = findSyncthingTempFileForPaths(mirrorBase, initialPaths);
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
  cleanStaleRebuildTemps(currentPath);
  for (let attempt = 0; attempt < 2; attempt++) {
    const relPaths = await zipEntryNames(currentPath);
    const dirHash = canonicalHashOfPaths(mirrorBase, relPaths);
    const zipHash = await canonicalHashOfZip(currentPath);
    if (dirHash === zipHash) return;
    const tmpPath = currentPath + '.rebuild-' + randomUUID() + '.tmp';
    await rebuildZipFromPaths(mirrorBase, relPaths, tmpPath);
    if ((await canonicalHashOfZip(currentPath)) !== zipHash) {
      // Concurrent upload replaced the archive mid-build; discard and retry
      // against the fresh state.
      fs.rmSync(tmpPath, { force: true });
      continue;
    }
    installRebuiltZip(userName, titleId, currentPath, tmpPath, dirHash);
    // Defensive: a rebuild only reaches here once extraction has already
    // happened at least once, so this should already be true, but a rebuild
    // must never leave rawMirrored unset (that would fall back to treating
    // the next full-deletion as "never mirrored" and resurrect the save).
    const postManifest = readManifest(userName);
    const postEntry = postManifest.games[titleId];
    if (postEntry && !postEntry.rawMirrored) {
      postEntry.rawMirrored = true;
      postManifest.updatedAt = new Date().toISOString();
      writeManifest(userName, postManifest);
    }
    return;
  }
  app.log.warn({ titleId }, 'raw mirror rebuild aborted after retries; serving stored archive');
}

/// Bring the per-title raw mirror directory and current.zip back in
/// agreement: extract the mirror when missing, and when it has drifted
/// (edited via Syncthing) rebuild the archive from it, rotating the
/// previous archive into versions/ first. Re-checks after building so a
/// concurrent upload is never overwritten by a stale rebuild (retries once).
async function syncRawMirrorWrapped(
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
  cleanStaleRebuildTemps(currentPath);
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
    installRebuiltZip(userName, titleId, currentPath, tmpPath, dirHash);
    return;
  }
  app.log.warn({ titleId }, 'raw mirror rebuild aborted after retries; serving stored archive');
}

/// Dispatches to the flat (RetroArch, shared root) or wrapped (everything
/// else, per-title folder) mirror sync, whichever applies to this title.
async function syncRawMirror(
  app: FastifyInstance,
  userName: string,
  titleId: string,
  currentPath: string,
  mirrorBase: string
): Promise<void> {
  if (isFlatMirrorTitle(titleId)) {
    await syncRawMirrorFlat(app, userName, titleId, currentPath, mirrorBase);
  } else {
    await syncRawMirrorWrapped(app, userName, titleId, currentPath, mirrorBase);
  }
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
        // Flat-mode titles need the OUTGOING zip's own path list before it's
        // rotated away, to remove any raw-mirror files that this version no
        // longer contains (extractMirrorFlat only ever writes, it never
        // deletes — mirrorBase is shared, so it can't safely delete on its
        // own without knowing which paths are this title's).
        const flatForPut = isFlatMirrorTitle(titleId);
        let outgoingFlatPaths: string[] = [];
        if (flatForPut && rawSavesDir() && fs.existsSync(currentPath)) {
          outgoingFlatPaths = await zipEntryNames(currentPath).catch(() => [] as string[]);
        }

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
          // `dir`/`currentPath` still point at the pre-rename location;
          // rebuild the canonical path from resolvedDir.
          const currentZip = path.join(path.dirname(dir), resolvedDir, 'current.zip');
          const flat = isFlatMirrorTitle(titleId);
          try {
            if (flat) {
              const entries = await zipEntryNames(currentZip);
              const stem = retroarchStemFromTitleId(titleId);
              const badEntry = stem === null ? undefined : entries.find((e) => !entryMatchesRetroarchStem(e, stem));
              if (badEntry) {
                // The flat mirror shares its root with other titles; never
                // write or delete there from a zip whose entries don't all
                // belong to this title. Canonical zip storage is unaffected.
                app.log.warn(
                  { titleId, entry: badEntry },
                  'raw mirror skipped: zip entry does not match the RetroArch title stem'
                );
              } else {
                cleanStaleFlatTemps(rawRoot, [...outgoingFlatPaths, ...entries]);
                await extractMirrorFlat(currentZip, rawRoot);
                // Remove mirror files this version dropped (e.g. a savestate
                // slot deleted on the Vita) — extraction only ever writes.
                const removedPaths = outgoingFlatPaths.filter((p) => !entries.includes(p));
                for (const rel of removedPaths) {
                  try {
                    fs.rmSync(path.join(rawRoot, rel), { force: true });
                  } catch (rmErr) {
                    app.log.warn(
                      { err: rmErr, titleId, file: rel },
                      'failed to remove stale raw mirror file'
                    );
                  }
                }
                const fresh = readManifest(userName);
                const freshEntry = fresh.games[titleId];
                if (freshEntry) {
                  let changed = false;
                  if (!freshEntry.rawMirrored) {
                    freshEntry.rawMirrored = true;
                    changed = true;
                  }
                  if (!contentHash) {
                    freshEntry.contentHash = canonicalHashOfPaths(rawRoot, entries);
                    changed = true;
                  }
                  if (changed) {
                    fresh.updatedAt = new Date().toISOString();
                    writeManifest(userName, fresh);
                  }
                }
              }
            } else {
              const rawDir = rawGameDir(resolvedDir);
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
            }
          } catch (err) {
            app.log.warn({ err, titleId }, 'raw mirror extraction failed; upload kept as-is');
            try {
              // A stale mirror would make the next sync "see" old/partial
              // files as drift and roll the fresh upload back. Remove it so
              // the next sync re-extracts from the uploaded zip instead.
              // Flat mode only removes this title's own known paths —
              // mirrorBase is shared with other titles, unlike the wrapped
              // folder.
              if (flat) {
                // Each removal is independently caught: one stubborn path
                // (e.g. a locked file) must not stop the rest from being
                // cleaned up, and must never prevent the rawMirrored reset
                // below — that reset is what actually matters for
                // correctness, the file removals are best-effort tidiness.
                const entries = await zipEntryNames(currentZip).catch(() => [] as string[]);
                for (const rel of entries) {
                  try {
                    fs.rmSync(path.join(rawRoot, rel), { force: true });
                  } catch (rmEntryErr) {
                    app.log.warn(
                      { err: rmEntryErr, titleId, file: rel },
                      'failed to remove one stale raw mirror file during recovery'
                    );
                  }
                }
                // Critical: also clear rawMirrored. Bootstrap ("never
                // mirrored yet, extract") is gated on this flag, not on
                // file existence — leaving it true after the cleanup above
                // would make the next sync treat these now-missing paths as
                // a user deletion and rebuild an empty/stale zip over the
                // upload we just kept.
                try {
                  const recovery = readManifest(userName);
                  const recoveryEntry = recovery.games[titleId];
                  if (recoveryEntry?.rawMirrored) {
                    recoveryEntry.rawMirrored = false;
                    writeManifest(userName, recovery);
                  }
                } catch (resetErr) {
                  app.log.warn({ err: resetErr, titleId }, 'failed to reset rawMirrored during recovery');
                }
              } else {
                fs.rmSync(rawGameDir(resolvedDir), { recursive: true, force: true });
              }
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
          const mirrorBase = isFlatMirrorTitle(titleId)
            ? rawRoot
            : rawGameDir(path.basename(path.dirname(currentPath)));
          data = await withTitleLock(titleId, async () => {
            await syncRawMirror(app, userName, titleId, currentPath, mirrorBase);
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
        const rawRoot = rawSavesDir();
        const flat = isFlatMirrorTitle(titleId);
        // Flat mode must capture this title's own known paths BEFORE the
        // zip is removed below — afterwards there is nothing left to open,
        // and mirrorBase is shared with other titles so we can only ever
        // remove exactly the paths that belonged to this title.
        let flatPaths: string[] = [];
        if (rawRoot && flat) {
          const currentPath = path.join(dir, 'current.zip');
          if (fs.existsSync(currentPath)) {
            flatPaths = await zipEntryNames(currentPath).catch(() => [] as string[]);
          }
        }

        fs.rmSync(dir, { recursive: true, force: true });

        if (rawRoot) {
          try {
            if (flat) {
              for (const rel of flatPaths) {
                fs.rmSync(path.join(rawRoot, rel), { force: true });
              }
            } else {
              fs.rmSync(rawGameDir(path.basename(dir)), { recursive: true, force: true });
            }
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
