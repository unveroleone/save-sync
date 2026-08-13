import { FastifyInstance } from 'fastify';
import { requireAuth, getUserName } from '../middleware/auth.js';
import { readManifest, writeManifest, countVersions, Manifest } from '../storage/disk.js';

export async function manifestRoutes(app: FastifyInstance): Promise<void> {
  app.get('/api/manifest', { preHandler: requireAuth }, async (_req, reply) => {
    const userName = getUserName();
    const manifest = readManifest(userName);
    for (const titleId of Object.keys(manifest.games)) {
      manifest.games[titleId].versionCount = countVersions(
        userName,
        titleId,
        manifest.games[titleId].dir,
      );
    }
    reply.send(manifest);
  });

  app.put<{ Body: Manifest }>(
    '/api/manifest',
    { preHandler: requireAuth },
    async (request, reply) => {
      const userName = getUserName();
      const existing = readManifest(userName);
      const updated: Manifest = {
        ...existing,
        ...request.body,
        userId: userName,
        updatedAt: new Date().toISOString(),
      };
      writeManifest(userName, updated);
      reply.send({ ok: true });
    }
  );
}
