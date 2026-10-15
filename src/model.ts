export type InputValue = string | { secretSlot: string; required: boolean };
export interface Script {
  sourcePath?: string | null;
  id: string; name: string; path: string; kind: string; description: string; tags: string[];
  favorite: boolean; runtimeId: string | null; environmentId: string | null; cwd: string;
  args: InputValue[]; env: Record<string, InputValue>; runMode: string; timeoutSeconds: number | null;
  encoding: string; successCodes: number[]; trustedHash: string | null; createdAt: string; updatedAt: string; exists: boolean;
  executionPolicy: string | null;
}
export interface Runtime {
  id: string; kind: string; executablePath: string; version: string; architecture: string; source: string;
  status: string; prefix: string | null; basePrefix: string | null; hasPip: boolean; checkedAt: string;
  executionPolicy?: string | null;
}
export interface Environment {
  id: string; baseRuntimeId: string; interpreterPath: string; ownership: string;
  requirementsPath: string | null; requirementsHash: string | null; dependencyStatus: string;
  createdAt: string; checkedAt: string;
}
export interface Run {
  id: string; scriptId: string | null; scriptName: string; scriptPath: string; runtimePath: string;
  runMode: string; state: string; reason: string | null; rootExitCode: number | null;
  startedAt: string; finishedAt: string | null; logTruncated: boolean; planRedacted: unknown;
}
export type RunEvent =
  | { type: 'state'; runId: string; state: string; reason?: string }
  | { type: 'output'; runId: string; seq: number; stream: string; text: string }
  | { type: 'gap'; runId: string; nextSeq: number }
  | { type: 'log_truncated'; runId: string }
  | { type: 'finished'; runId: string; state: string; rootExitCode?: number };
export interface LogPage { text: string; nextCursor: number; hasMore: boolean }
export interface Settings {
  maxConcurrent: number; retentionDays: number; logLimitMib: number; totalLogMib: number;
  defaultMode: string; defaultEncoding: string;
}
export interface InstallJob {
  id: string; kind: string; target: string; state: string; message: string; rawExitCode: number | null;
  createdAt: string; finishedAt: string | null; environmentId: string | null; scriptId: string | null;
}
export interface InstallPlan {
  id: string; recipeKey: string; product: string; version: string; architecture: string; scope: string;
  source: string; packageId: string; licenseUrl: string; actions: string[]; warning: string;
}
export interface AppInfo { version: string; dataDir: string; portable: boolean; elevated: boolean }
export const activeStates = new Set(['starting', 'running', 'waiting_children', 'stopping']);
export const pendingInstallStates = new Set(['planning', 'awaiting_confirmation', 'installing', 'verifying', 'creating', 'verification_required', 'reboot_required']);
export const stateNames: Record<string, string> = {
  starting: '正在启动', running: '运行中', waiting_children: '等待子进程', stopping: '正在停止',
  succeeded: '运行成功', success: '运行成功', failed: '失败', stopped: '已停止', cancelled: '已取消',
  timed_out: '已超时', interrupted: '异常中断', start_failed: '启动失败', spawn_failed: '启动失败', ready: '已就绪',
  verified: '可用', available: '可用', unavailable: '不可用', missing: '未找到', detected: '已发现',
  installing: '正在安装', verifying: '正在复检', verification_required: '结果待复检', reboot_required: '需要重启',
  creating: '正在创建', unknown: '待确认', not_installed: '未安装依赖', clean: '可用', needs_recheck: '需要复检',
};
export const kindNames: Record<string, string> = { cmd: 'CMD', powershell: 'PowerShell', windows_powershell: 'Windows PowerShell', python: 'Python' };
export const labelState = (state: string) => stateNames[state] || state;
export const matchesRunState = (state: string, filter: string) => !filter || state === filter || (filter === 'cancelled' && state === 'stopped');
export function selectTaskRuns<T extends { state: string }>(runs: T[]): T[] {
  const active = runs.filter(run => activeStates.has(run.state));
  return [...active, ...runs.filter(run => !activeStates.has(run.state)).slice(0, Math.max(0, 100 - active.length))];
}
export function compatibleRuntime(scriptKind: string, runtimeKind: string): boolean {
  return scriptKind === runtimeKind || (scriptKind === 'powershell' && runtimeKind === 'windows_powershell');
}
export function filterScripts(scripts: Script[], search: string, kind: string, tag: string, favorite: boolean, fileState: string): Script[] {
  const query = search.trim().toLocaleLowerCase();
  return scripts.filter(s => (!query || [s.name, s.path, s.description, ...s.tags].join(' ').toLocaleLowerCase().includes(query))
    && (!kind || s.kind === kind) && (!tag || s.tags.includes(tag)) && (!favorite || s.favorite)
    && (!fileState || s.exists === (fileState === 'exists')));
}
export function limitLog(text: string): string {
  const cut = text.length > 1_000_000 ? text.slice(-1_000_000) : text;
  const lines = cut.split('\n');
  return lines.length > 10_000 ? lines.slice(-10_000).join('\n') : cut;
}
export function parseCodes(value: string): number[] {
  if (!value.trim()) throw new Error('至少填写一个成功退出码。');
  const values = value.split(/[,，\s]+/).filter(Boolean);
  if (values.some(v => !/^\d+$/.test(v) || !Number.isSafeInteger(Number(v)) || Number(v) > 4294967295)) throw new Error('退出码必须为 0 至 4294967295 的整数。');
  return [...new Set(values.map(Number))];
}
export function secretSlots(script: Script): { name: string; required: boolean }[] {
  const slots = new Map<string, boolean>();
  for (const value of [...script.args, ...Object.values(script.env)]) {
    if (typeof value !== 'string') slots.set(value.secretSlot, (slots.get(value.secretSlot) ?? false) || value.required);
  }
  return [...slots].map(([name, required]) => ({ name, required }));
}
export function runEncoding(run: Pick<Run, 'planRedacted'> | undefined): string {
  const plan = run?.planRedacted;
  const encoding = plan && typeof plan === 'object' && 'encoding' in plan ? plan.encoding : null;
  return typeof encoding === 'string' && ['utf-8', 'system', 'gb18030'].includes(encoding) ? encoding : 'utf-8';
}
