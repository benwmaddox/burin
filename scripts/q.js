// One query's first column, a line per row: node scripts/q.js app.sqlite "select …"
process.removeAllListeners('warning');
const { DatabaseSync } = require('node:sqlite');
const d = new DatabaseSync(process.argv[2], { readOnly: true });
for (const r of d.prepare(process.argv[3]).all()) console.log(Object.values(r)[0]);
