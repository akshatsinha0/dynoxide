/**
 * Bridge between dynoxide's wasm-bindgen backend and wa-sqlite.
 *
 * Exposes three async functions - `open`, `exec`, `query` - consumed by
 * `src/storage_backend/wasm_backend.rs` through a `#[wasm_bindgen]` extern
 * block. The Rust side builds every SQL statement (shared with the native
 * backend via `sql_builders`) and hands it here with a positional parameter
 * array; this module only opens the database and runs statements.
 *
 * Runs inside the dynoxide Web Worker (see js/dynoxide-worker.js). wa-sqlite's
 * OPFS VFS persists through sync access handles, which browsers expose only in
 * a Worker, so the engine runs in the Worker and the page talks to it over a
 * coarse message RPC. No cross-origin isolation (COOP/COEP) is required.
 *
 * Preview packaging: wa-sqlite is imported by absolute `/node_modules` path so
 * the Worker resolves it without a bundler or import map. A production/SDK
 * build would bundle the Worker and import wa-sqlite by bare specifier. This
 * backend is not exercised by the conformance suite (see the WASM note in the
 * README).
 */

import * as SQLite from "/node_modules/wa-sqlite/src/sqlite-api.js";
import SQLiteESMFactory from "/node_modules/wa-sqlite/dist/wa-sqlite-async.mjs";

// Lazily initialised SQLite API handle, shared across opens.
let sqlite3 = null;

async function moduleHandle() {
  if (sqlite3) return sqlite3;
  const module = await SQLiteESMFactory();
  sqlite3 = SQLite.Factory(module);

  // OPFS VFS via sync access handles (Worker-only), registered as the default
  // so open_v2 uses it. Persists to the origin private file system.
  const { OriginPrivateFileSystemVFS } = await import(
    "/node_modules/wa-sqlite/src/examples/OriginPrivateFileSystemVFS.js"
  );
  const vfs = new OriginPrivateFileSystemVFS();
  sqlite3.vfs_register(vfs, true);
  return sqlite3;
}

/**
 * Open (or create) a database persisted under `name`.
 * Returns an opaque handle passed back to `exec`/`query`.
 */
export async function open(name) {
  const s = await moduleHandle();
  const db = await s.open_v2(name);

  // Register fnv1a_hash for GSI/LSI parallel-scan segment filtering, matching
  // the native scalar function: FNV-1a (32-bit) over the value's UTF-8 bytes,
  // returned as an integer so `fnv1a_hash(col) % total` is integer modulo.
  s.create_function(
    db,
    "fnv1a_hash",
    1,
    SQLite.SQLITE_UTF8,
    0,
    (context, values) => {
      const text = s.value(values[0]);
      const bytes = new TextEncoder().encode(typeof text === "string" ? text : "");
      let hash = 0x811c9dc5;
      for (const b of bytes) {
        hash ^= b;
        hash = Math.imul(hash, 0x01000193) >>> 0;
      }
      s.result(context, BigInt(hash >>> 0));
    },
    null,
    null,
  );

  return { db };
}

/**
 * Execute a statement that returns no rows (DDL, INSERT, DELETE, BEGIN/COMMIT).
 * `params` is a positional array binding `?1`, `?2`, ... in order.
 */
export async function exec(handle, sql, params) {
  const s = sqlite3;
  for await (const stmt of s.statements(handle.db, sql)) {
    if (params && params.length) s.bind_collection(stmt, params);
    while ((await s.step(stmt)) === SQLite.SQLITE_ROW) {
      // exec consumes no rows
    }
  }
}

/**
 * Run a query and return its rows.
 * Each row is an array of column values in SELECT order.
 */
export async function query(handle, sql, params) {
  const s = sqlite3;
  const rows = [];
  for await (const stmt of s.statements(handle.db, sql)) {
    if (params && params.length) s.bind_collection(stmt, params);
    while ((await s.step(stmt)) === SQLite.SQLITE_ROW) {
      rows.push(s.row(stmt));
    }
  }
  return rows;
}
