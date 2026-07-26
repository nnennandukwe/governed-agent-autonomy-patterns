import { randomUUID } from 'node:crypto';
import {
  mkdir,
  rename,
  rm,
  stat,
  writeFile,
} from 'node:fs/promises';
import path from 'node:path';

import { canonicalJson } from './canonical.js';
import type { TrialReceipt } from './types.js';

export class AtomicEvidenceSink {
  constructor(private readonly root: string) {}

  async write(receipt: TrialReceipt): Promise<string> {
    await mkdir(this.root, { recursive: true });
    const finalPath = path.join(this.root, receipt.runId);
    const stagingPath = path.join(
      this.root,
      `.${receipt.runId}.staging-${randomUUID()}`,
    );

    let finalPathExists = false;
    try {
      await stat(finalPath);
      finalPathExists = true;
    } catch (error) {
      if (
        !(error instanceof Error)
        || !('code' in error)
        || error.code !== 'ENOENT'
      ) {
        throw error;
      }
    }
    if (finalPathExists) {
      throw new Error(
        `Evidence for run ${receipt.runId} already exists at ${finalPath}.`,
      );
    }

    await mkdir(stagingPath, { recursive: false });
    try {
      await writeFile(
        path.join(stagingPath, 'receipt.json'),
        canonicalJson(receipt),
        { flag: 'wx' },
      );
      await rename(stagingPath, finalPath);
      return finalPath;
    } catch (error) {
      await rm(stagingPath, { recursive: true, force: true });
      throw error;
    }
  }
}
