// SEC-008: every port the test servers publish is bound to 127.0.0.1, so a
// developer machine or CI runner never exposes the (weak-password) servers.
// Usage: node scripts/ci/check-compose-ports.mjs [compose.yaml]
import { readFileSync } from "node:fs";

const file = process.argv[2] ?? "docker/test-servers/compose.yaml";
const lines = readFileSync(file, "utf8").split(/\r?\n/);

const bad = [];
let found = 0;
for (let i = 0; i < lines.length; i++) {
  const m = lines[i].match(/^(\s*)ports:\s*(.*)$/);
  if (!m) continue;
  const indent = m[1].length;
  // Inline list: ports: ["127.0.0.1:1:1", …]
  const inline = m[2].trim();
  const items = [];
  if (inline.startsWith("[")) {
    items.push(...inline.slice(1, -1).split(",").map((s) => s.trim()).filter(Boolean));
  } else {
    for (let j = i + 1; j < lines.length; j++) {
      const l = lines[j];
      if (!l.trim() || l.trim().startsWith("#")) continue;
      const lead = l.length - l.trimStart().length;
      if (lead <= indent) break;
      const item = l.trim().match(/^-\s*(.*)$/);
      if (!item) break;
      items.push(item[1]);
    }
  }
  for (const raw of items) {
    found++;
    const spec = raw.replace(/\s+#.*$/, "").replace(/^["']|["']$/g, "");
    // The long syntax (`- target: 22`) has no host IP unless `host_ip:` is set: refuse it.
    if (!spec.startsWith("127.0.0.1:")) bad.push(`${file}:${i + 1}: ${spec}`);
  }
}

if (found === 0) {
  console.error(`no ports found in ${file}`);
  process.exit(1);
}
if (bad.length) {
  console.error("ports not bound to 127.0.0.1:\n" + bad.join("\n"));
  process.exit(1);
}
console.log(`${found} published ports, all on 127.0.0.1`);
