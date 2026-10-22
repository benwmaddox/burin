// Runs SQL that writes, on a data copy (a test's setup): node scripts/x.js app.sqlite "insert …"
process.removeAllListeners('warning');
const { DatabaseSync } = require('node:sqlite');
new DatabaseSync(process.argv[2]).exec(process.argv[3]);
