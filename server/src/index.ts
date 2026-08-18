import Fastify from 'fastify';
import multipart from '@fastify/multipart';
import path from 'path';
import { statusRoutes } from './routes/status.js';
import { authRoutes } from './routes/auth.js';
import { manifestRoutes } from './routes/manifest.js';
import { savesRoutes } from './routes/saves.js';
import { devicesRoutes } from './routes/devices.js';
import { rawSavesDir, savesDir } from './storage/disk.js';
import { getUserName } from './middleware/auth.js';

const app = Fastify({ logger: true });

// Misconfigured RAW_SAVES_DIR inside the saves tree would make the mirror
// swap delete the canonical zip folder; refuse loudly instead of later.
const raw = rawSavesDir();
if (raw) {
  const saves = savesDir(getUserName());
  const inside = (parent: string, child: string) =>
    path.relative(parent, child) === '' ||
    (!path.relative(parent, child).startsWith('..') && !path.isAbsolute(path.relative(parent, child)));
  if (inside(saves, raw) || inside(raw, saves)) {
    app.log.error(
      `RAW_SAVES_DIR (${raw}) must not be the same as, or nested inside, the saves dir (${saves}). ` +
        'Point it at a separate directory.'
    );
    process.exit(1);
  }
}

app.register(multipart, {
  limits: {
    fileSize: 256 * 1024 * 1024, // 256 MB per save zip
  },
});

app.register(statusRoutes);
app.register(authRoutes);
app.register(manifestRoutes);
app.register(savesRoutes);
app.register(devicesRoutes);

const port = parseInt(process.env.PORT ?? '3000', 10);

app.listen({ port, host: '0.0.0.0' }, (err) => {
  if (err) {
    app.log.error(err);
    process.exit(1);
  }
  app.log.info(`Save Sync server listening on port ${port}`);
});
