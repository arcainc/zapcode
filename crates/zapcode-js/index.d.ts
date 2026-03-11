export interface ZapcodeOptions {
  inputs?: string[];
  externalFunctions?: string[];
  memoryLimitMb?: number;
  timeLimitMs?: number;
}

export interface ZapcodeResult {
  completed: true;
  output: unknown;
  stdout: string;
}

export interface ZapcodeSuspension {
  completed: false;
  functionName: string;
  args: unknown[];
  snapshot: Buffer;
}

export class ZapcodeSnapshotHandle {
  static load(bytes: Buffer): ZapcodeSnapshotHandle;
  dump(): Buffer;
  resume(returnValue: unknown): ZapcodeResult | ZapcodeSuspension;
}

export class Zapcode {
  constructor(code: string, options?: ZapcodeOptions);
  run(inputs?: Record<string, unknown>): ZapcodeResult;
  start(inputs?: Record<string, unknown>): ZapcodeResult | ZapcodeSuspension;
}
