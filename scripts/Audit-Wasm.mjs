// Inspect a user-supplied binary without instantiating or executing the engine.
import { readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';

const path = process.argv[2];
if (!path) throw Error('Uso: node scripts/Audit-Wasm.mjs <game.wasm>');
const bytes = await readFile(path);
let at = 8, limit = bytes.length;
function leb() {
  let n = 0n, shift = 0n;
  for (let i = 0; i < 10 && at < limit; i++) {
    const b = bytes[at++]; n |= BigInt(b & 127) << shift;
    if (!(b & 128)) return n;
    shift += 7n;
  }
  throw Error('LEB inválido');
}
function name() {
  const n = Number(leb());
  if (at + n > limit) throw Error('Nome fora da seção');
  const text = bytes.toString('utf8', at, at + n); at += n; return text;
}
const module = await WebAssembly.compile(bytes), memories = [];
while (at < bytes.length) {
  const section = bytes[at++], size = Number(leb()), end = at + size;
  if (end > bytes.length) throw Error('Seção inválida');
  if (section === 2) {
    limit = end;
    for (let n = Number(leb()); n > 0; n--) {
      const module = name(), field = name(), kind = bytes[at++];
      if (kind === 0) leb();
      else if (kind === 2) {
        const flags = Number(leb()), min = leb(), max = flags & 1 ? leb() : null;
        memories.push({ module, name: field, memory64: !!(flags & 4), shared: !!(flags & 2), minimum_bytes: Number(min * 65536n), maximum_bytes: max === null ? null : Number(max * 65536n) });
      } else throw Error('Tipo de importação fora do contrato auditado: ' + kind);
    }
    break;
  }
  at = end;
}
const imports = WebAssembly.Module.imports(module);
console.log(JSON.stringify({ sha256: createHash('sha256').update(bytes).digest('hex'), bytes: bytes.length, imports: imports.length, functions: imports.filter(i => i.kind === 'function'), memories, exports: WebAssembly.Module.exports(module), instantiated: false }, null, 2));
