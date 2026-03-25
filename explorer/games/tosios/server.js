// ESM wrapper for the TOSIOS server
// Delegates to server.cjs (CommonJS) since the bundled server code uses require()
import { createRequire } from 'module';
import { fileURLToPath } from 'url';
import { dirname, join } from 'path';

const require = createRequire(import.meta.url);
const __dirname = dirname(fileURLToPath(import.meta.url));

process.env.PORT = process.env.PORT || '8407';

// Load the CJS entry point
require(join(__dirname, 'server.cjs'));
