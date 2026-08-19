import { FastifyInstance } from 'fastify';
import { rawSavesDir } from '../storage/disk.js';

export async function statusRoutes(app: FastifyInstance): Promise<void> {
  app.get('/api/status', async (_req, reply) => {
    const features = ['manifest', 'upload', 'download', 'history'];
    if (rawSavesDir()) {
      features.push('raw-saves');
    }
    reply.send({
      ok: true,
      serverVersion: '0.1.8',
      features,
    });
  });
}
