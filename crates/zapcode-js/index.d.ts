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
  /** Captured stdout (`console.log`/`info`/`debug`). */
  stdout: string;
  /** Captured stderr (`console.error`/`console.warn`). */
  stderr: string;
}

export interface ZapcodeSuspension {
  kind: "suspended";
  completed: false;
  functionName: string;
  args: unknown[];
  /** stdout (`console.log`/`info`/`debug`) so far — cumulative up to this
   * suspension (console output carries across snapshot restores). */
  stdout: string;
  /** stderr (`console.error`/`console.warn`) so far — cumulative. */
  stderr: string;
  snapshot: Buffer;
}

export interface ExternalCall {
  name: string;
  args: unknown[];
}

/**
 * Which `Promise` combinator produced a deferred batch. The host settles the
 * batched calls with the matching `Promise.*` and resumes accordingly.
 */
export type PromiseCombinator = "all" | "race" | "any" | "allSettled";

export interface ZapcodeBatchSuspension {
  kind: "suspended_many";
  completed: false;
  /**
   * Which `Promise` combinator produced this batch. Resume with:
   * - all: resumeMany(results) | resumeError(firstFailure)
   * - allSettled: resumeMany(settledObjects) — never rejects
   * - race/any: resumeMany([singleValue]) | resumeError(reason)
   */
  combinator: PromiseCombinator;
  /** The batched external calls, in order — run them in parallel. */
  calls: ExternalCall[];
  /** stdout (`console.log`/`info`/`debug`) so far — cumulative up to this
   * suspension (console output carries across snapshot restores). */
  stdout: string;
  /** stderr (`console.error`/`console.warn`) so far — cumulative. */
  stderr: string;
  snapshot: Buffer;
}

export interface ZapcodeSessionResult {
  kind: "complete";
  completed: true;
  output: unknown;
  stdout: string;
  /** Captured stderr (`console.error`/`console.warn`) for this step. */
  stderr: string;
  session: Buffer;
}

export interface ZapcodeSessionSuspension {
  kind: "suspended";
  completed: false;
  functionName: string;
  args: unknown[];
  stdout: string;
  /** Captured stderr (`console.error`/`console.warn`) for this step. */
  stderr: string;
  session: Buffer;
}

export interface ZapcodeSessionBatchSuspension {
  kind: "suspended_many";
  completed: false;
  /** Which `Promise` combinator produced this batch. */
  combinator: PromiseCombinator;
  /** The batched external calls, in order — run them in parallel. */
  calls: ExternalCall[];
  stdout: string;
  /** Captured stderr (`console.error`/`console.warn`) for this step. */
  stderr: string;
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
   * Resume by raising a real Error OBJECT (name/message, with
   * `e instanceof Error` true) — the faithful shape of a host tool that threw,
   * so guest `catch (e) { e.message }` works. `name` defaults to "Error".
   */
  resumeErrorObject(
    message: string,
    name?: string,
  ): ZapcodeResult | ZapcodeSuspension | ZapcodeBatchSuspension;
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
   * Resume by raising a real Error OBJECT (name/message, `e instanceof Error`
   * true) — the faithful shape of a host tool that threw. `name` defaults to
   * "Error".
   */
  resumeErrorObject(
    message: string,
    name?: string,
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

/**
 * A parsed + compiled program, ready to run many times without re-paying
 * parse + compile. Compile once, then `run`/`start` with different inputs; the
 * compiled bytecode can be persisted with `dump()` and reloaded with `load()`.
 * Memory / time limits are baked in at compile time (and re-supplied at load).
 */
export class ZapcodeProgramHandle {
  static compile(code: string, options?: ZapcodeOptions): ZapcodeProgramHandle;
  static load(bytes: Buffer, options?: ZapcodeOptions): ZapcodeProgramHandle;
  dump(): Buffer;
  run(inputs?: Record<string, unknown>): ZapcodeResult;
  start(inputs?: Record<string, unknown>): ZapcodeResult | ZapcodeSuspension | ZapcodeBatchSuspension;
}
