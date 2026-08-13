import { FastifyInstance, FastifyRequest, FastifyReply } from 'fastify';
import { requireAuth, getUserName } from '../middleware/auth.js';
import {
  titleDir,
  ensureDir,
  readManifest,
  writeManifest,
  sanitizeTitle,
  GameEntry,
} from '../storage/disk.js';
import fs from 'fs';
import path from 'path';
import crypto from 'crypto';

interface SaveParams {
  titleId: string;
}

function computeSha256(filePath: string): string {
  const hash = crypto.createHash('sha256');
  hash.update(fs.readFileSync(filePath));
  return 'sha256:' + hash.digest('hex');
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

      const tmpPath = path.join(dir, `upload_${Date.now()}.tmp`);
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
      const actualHash = computeSha256(tmpPath);
      if (clientHash && clientHash !== actualHash) {
        fs.unlinkSync(tmpPath);
        return reply
          .code(400)
          .send({ ok: false, error: `Hash mismatch: expected ${clientHash}, got ${actualHash}` });
      }

      // rotate current → versions/
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

      const data = fs.readFileSync(currentPath);
      reply
        .header('Content-Type', 'application/zip')
        .header('Content-Disposition', `attachment; filename="${titleId}.zip"`)
        .header('X-Save-Hash', entry?.latestHash ?? '')
        .header('X-Save-Timestamp', entry?.latestVersion ?? '')
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

      fs.rmSync(dir, { recursive: true, force: true });

      const manifest = readManifest(userName);
      delete manifest.games[titleId];
      manifest.updatedAt = new Date().toISOString();
      writeManifest(userName, manifest);

      reply.send({ ok: true, titleId });
    }
  );
}
