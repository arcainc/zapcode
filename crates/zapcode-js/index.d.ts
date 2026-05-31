export interface ZapcodeOptions {
  inputs?: string[];
  externalFunctions?: string[];
  memoryLimitMb?: number;
  timeLimitMs?: number;
}

export interface ZapcodeSessionOptions {
  externalFunctions?: string[];
  memoryLimitMb?: number;
  timeLimitMs?: number;
}

export interface ZapcodeResult {
  kind: "complete";
  completed: true;
  output: unknown;
  stdout: string;
}

export interface ZapcodeSuspension {
  kind: "suspended";
  completed: false;
  functionName: string;
  args: unknown[];
  snapshot: Buffer;
}

export interface ExternalCall {
  name: string;
  args: unknown[];
}

export interface ZapcodeBatchSuspension {
  kind: "suspended_many";
  completed: false;
  /** The batched external calls, in order — run them in parallel. */
  calls: ExternalCall[];
  snapshot: Buffer;
}

export interface ZapcodeSessionResult {
  kind: "complete";
  completed: true;
  output: unknown;
  stdout: string;
  session: Buffer;
}

export interface ZapcodeSessionSuspension {
  kind: "suspended";
  completed: false;
  functionName: string;
  args: unknown[];
  stdout: string;
  session: Buffer;
}

export interface ZapcodeSessionBatchSuspension {
  kind: "suspended_many";
  completed: false;
  /** The batched external calls, in order — run them in parallel. */
  calls: ExternalCall[];
  stdout: string;
  session: Buffer;
}

export class ZapcodeSnapshotHandle {
  static load(bytes: Buffer): ZapcodeSnapshotHandle;
  dump(): Buffer;
  resume(returnValue: unknown): ZapcodeResult | ZapcodeSuspension | ZapcodeBatchSuspension;
  /**
   * Resume by raising an error at the suspended external call instead of
   * returning a value (a failed tool / activity). Catchable by a surrounding
   * try/catch in the guest; otherwise it propagates as an execution error.
   */
  resumeError(error: unknown): ZapcodeResult | ZapcodeSuspension | ZapcodeBatchSuspension;
  /**
   * Resume a batch suspension (Promise.all) with one result per call, in the
   * order the calls were presented. Run the calls in parallel on the host.
   */
  resumeMany(results: unknown[]): ZapcodeResult | ZapcodeSuspension | ZapcodeBatchSuspension;
}

export class ZapcodeSessionHandle {
  static create(options?: ZapcodeSessionOptions): ZapcodeSessionHandle;
  static load(bytes: Buffer): ZapcodeSessionHandle;
  dump(): Buffer;
  runChunk(
    code: string,
    inputs?: Record<string, unknown>,
  ): ZapcodeSessionResult | ZapcodeSessionSuspension | ZapcodeSessionBatchSuspension;
  resume(
    returnValue: unknown,
  ): ZapcodeSessionResult | ZapcodeSessionSuspension | ZapcodeSessionBatchSuspension;
  /**
   * Resume by raising an error at the suspended external call instead of
   * returning a value (a failed tool / activity). Catchable by a surrounding
   * try/catch in the chunk; otherwise it propagates.
   */
  resumeError(
    error: unknown,
  ): ZapcodeSessionResult | ZapcodeSessionSuspension | ZapcodeSessionBatchSuspension;
  /**
   * Resume a batch suspension (Promise.all) with one result per call, in order.
   * Run the calls in parallel on the host.
   */
  resumeMany(
    results: unknown[],
  ): ZapcodeSessionResult | ZapcodeSessionSuspension | ZapcodeSessionBatchSuspension;
}

export class Zapcode {
  constructor(code: string, options?: ZapcodeOptions);
  run(inputs?: Record<string, unknown>): ZapcodeResult;
  start(inputs?: Record<string, unknown>): ZapcodeResult | ZapcodeSuspension | ZapcodeBatchSuspension;
}
