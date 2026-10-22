// The shape of a signed-in cookie (its parts' lengths and separators, never its value):
//   node bench/mix/cookie-shape.js cookies.json
const c = require(process.argv[2]);
const v = Object.values(c)[0].cookie;
console.log(v.split(";")[0].replace(/[^=;%-]+/g, (m) => `<${m.length}>`));
