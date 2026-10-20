/** A tiny key-value store over IndexedDB: one database, one object store,
 * values by string key. Reads answer undefined when the store works but
 * holds nothing, and null when the store is unavailable; writes answer
 * false when they cannot land. The app works wherever IndexedDB does not.
 * Tests inject an in-memory KeyValue instead. */

export interface KeyValue {
  get(key: string): Promise<unknown>;
  /** Reads the stored value, applies `change`, and stores what it returns —
   * one transaction, so concurrent updates serialize. False when the store
   * is unavailable or the change throws. */
  update(key: string, change: (stored: unknown) => unknown): Promise<boolean>;
}

const DATABASE = 'sokomind-rust';
const STORE = 'v1';

/** The database connection, once opened; a failure stays failed until the
 * next call retries it. */
let connection: Promise<IDBDatabase> | undefined;

function open(): Promise<IDBDatabase> {
  if (connection === undefined)
    connection = new Promise((resolve, reject) => {
      const request = indexedDB.open(DATABASE, 1);
      request.onupgradeneeded = () => {
        const db = request.result;
        if (!db.objectStoreNames.contains(STORE)) db.createObjectStore(STORE);
      };
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
  return connection;
}

export const idb: KeyValue = {
  async get(key: string): Promise<unknown> {
    try {
      const db = await open();
      return await new Promise((resolve, reject) => {
        const request = db.transaction(STORE).objectStore(STORE).get(key);
        request.onsuccess = () => resolve(request.result);
        request.onerror = () => reject(request.error);
      });
    } catch {
      return null;
    }
  },
  async update(key: string, change: (stored: unknown) => unknown): Promise<boolean> {
    try {
      const db = await open();
      return await new Promise((resolve, reject) => {
        const transaction = db.transaction(STORE, 'readwrite');
        const store = transaction.objectStore(STORE);
        const read = store.get(key);
        read.onsuccess = () => {
          try {
            store.put(change(read.result), key);
          } catch (error) {
            reject(error);
          }
        };
        transaction.oncomplete = () => resolve(true);
        transaction.onabort = () => reject(transaction.error);
        transaction.onerror = () => reject(transaction.error);
      });
    } catch {
      return false;
    }
  },
};
