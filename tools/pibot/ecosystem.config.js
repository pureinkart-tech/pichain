// Load .env file — all secrets live there, NOT here
require('dotenv').config({ path: require('path').join(__dirname, '.env') });

module.exports = {
  apps: [{
    name: 'pibot',
    script: 'bot.js',
    cwd: __dirname,
    env: {
      CHAIN_ID: '31415',
      ADMIN_TELEGRAM_ID: '0',
      // All sensitive keys loaded from .env automatically
    },
    autorestart: true,
    max_restarts: 10,
    restart_delay: 3000,
  }]
};
