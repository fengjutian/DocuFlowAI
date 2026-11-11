import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { join } from "node:path";

export class RecordConflictError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "RecordConflictError";
  }
}

export interface RecordStore<T extends { id: string }> {
  put(record: T): Promise<void>;
  get(id: string): Promise<T | undefined>;
  list(): Promise<T[]>;
}

/**
 * One JSON file per record, created with `wx` so an existing id is a conflict rather than an
 * overwrite. Snapshot records are evidence: updating one would erase the state an execution ran
 * against, which design 9.5 and 15 both forbid.
 */
export class ImmutableRecordStore<T extends { id: string }> implements RecordStore<T> {
  constructor(
    private readonly root: string,
    private readonly collection: string,
    private readonly validate?: (record: T) => void
  ) {}

  async put(record: T): Promise<void> {
    this.validate?.(record);
    if (!record.id || !/^[a-zA-Z0-9._-]+$/.test(record.id)) {
      throw new RecordConflictError(`Record id '${record.id}' is not safe as a file name`);
    }
    await mkdir(join(this.root, this.collection), { recursive: true });
    try {
      await writeFile(this.pathFor(record.id), `${JSON.stringify(record, null, 2)}\n`, {
        encoding: "utf8",
        flag: "wx"
      });
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "EEXIST") {
        throw new RecordConflictError(
          `Record ${record.id} already exists in ${this.collection}; records are immutable`
        );
      }
      throw error;
    }
  }

  async get(id: string): Promise<T | undefined> {
    if (!/^[a-zA-Z0-9._-]+$/.test(id)) {
      throw new RecordConflictError(`Record id '${id}' is not safe as a file name`);
    }
    try {
      return JSON.parse(await readFile(this.pathFor(id), "utf8")) as T;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
      throw error;
    }
  }

  async list(): Promise<T[]> {
    let names: string[];
    try {
      names = await readdir(join(this.root, this.collection));
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT") return [];
      throw error;
    }
    const records: T[] = [];
    for (const name of names.filter(entry => entry.endsWith(".json")).sort()) {
      const record = await this.get(name.slice(0, -".json".length));
      if (record) records.push(record);
    }
    return records;
  }

  private pathFor(id: string): string {
    return join(this.root, this.collection, `${id}.json`);
  }
}
