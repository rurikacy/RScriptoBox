import { Channel, invoke, isTauri } from '@tauri-apps/api/core';
import { open, save } from '@tauri-apps/plugin-dialog';
import type { AppInfo, Environment, InstallJob, InstallPlan, LogPage, Run, RunEvent, Runtime, Script, Settings } from './model';

export const desktop = isTauri();
const date = new Date().toISOString();
const base = { description: '', tags: [], favorite: false, runtimeId: null, environmentId: null, cwd: '', args: [], env: {}, runMode: 'terminal', timeoutSeconds: null, encoding: 'utf-8', successCodes: [0], trustedHash: null, createdAt: date, updatedAt: date, exists: true, executionPolicy: null };
const samples: Script[] = [
  { ...base, id: 'preview-1', name: '整理下载文件夹', path: 'D:\\Scripts\\organize_downloads.py', kind: 'python', description: '按文件类型归档下载目录，让每一份文件都有归处。', tags: ['效率工具', '文件整理'], favorite: true, runtimeId: 'python' },
  { ...base, id: 'preview-2', name: '开发环境检查', path: 'D:\\Scripts\\check_environment.ps1', kind: 'powershell', description: '一键检查 Git、Node.js 与常用开发工具的版本。', tags: ['开发', '环境检测'], favorite: true, runtimeId: 'ps7' },
  { ...base, id: 'preview-3', name: '项目一键构建', path: 'D:\\Scripts\\build_project.cmd', kind: 'cmd', description: '清理临时文件并构建项目，保持发布目录整洁。', tags: ['开发', '构建'], runtimeId: 'cmd' },
  { ...base, id: 'preview-4', name: '每周数据备份', path: 'D:\\Scripts\\weekly_backup.py', kind: 'python', description: '为工作目录创建带时间戳的压缩备份。', tags: ['备份', '自动化'], favorite: true, runtimeId: 'python' },
  { ...base, id: 'preview-5', name: '网络连接诊断', path: 'D:\\Scripts\\network_diagnostics.ps1', kind: 'powershell', description: '快速查看本机网络配置、DNS 与连接状态。', tags: ['系统工具'], runtimeId: 'ps7' },
  { ...base, id: 'preview-6', name: '清理临时缓存', path: 'D:\\Scripts\\clean_cache.bat', kind: 'cmd', description: '管理本地开发工具的临时目录与过期缓存。', tags: ['系统工具', '清理'], exists: false, runtimeId: 'cmd' },
];
const runtimes: Runtime[] = [
  { id: 'cmd', kind: 'cmd', executablePath: 'C:\\Windows\\System32\\cmd.exe', version: '10.0', architecture: 'x64', source: 'system', status: 'ready', prefix: null, basePrefix: null, hasPip: false, checkedAt: date },
  { id: 'ps7', kind: 'powershell', executablePath: 'C:\\Program Files\\PowerShell\\7\\pwsh.exe', version: '7.5', architecture: 'x64', source: 'discovered', status: 'ready', prefix: null, basePrefix: null, hasPip: false, checkedAt: date },
  { id: 'python', kind: 'python', executablePath: 'C:\\Users\\User\\AppData\\Local\\Python\\python.exe', version: '3.14', architecture: 'x64', source: 'discovered', status: 'ready', prefix: null, basePrefix: null, hasPip: true, checkedAt: date },
];
const defaults: Settings = { maxConcurrent: 4, retentionDays: 30, logLimitMib: 20, totalLogMib: 500, defaultMode: 'terminal', defaultEncoding: 'system' };
export async function command<T>(name: string, args?: Record<string, unknown>): Promise<T> {
  if (!desktop) throw new Error('当前为浏览器界面预览。请在 RScriptoBox 桌面客户端中使用此功能。');
  return invoke<T>(name, args);
}
export const api = {
  scripts: () => desktop ? command<Script[]>('list_scripts') : Promise.resolve(structuredClone(samples)),
  runtimes: () => desktop ? command<Runtime[]>('list_runtimes') : Promise.resolve(structuredClone(runtimes)),
  environments: () => desktop ? command<Environment[]>('list_environments') : Promise.resolve([]),
  runs: () => desktop ? command<Run[]>('list_runs') : Promise.resolve([]),
  installs: () => desktop ? command<InstallJob[]>('list_install_jobs') : Promise.resolve([]),
  settings: () => desktop ? command<Settings>('get_settings') : Promise.resolve({ ...defaults }),
  info: () => desktop ? command<AppInfo>('app_info') : Promise.resolve({ version: '0.2.0', dataDir: '浏览器预览 · 示例数据', portable: false, elevated: false }),
  plan: (recipe: string, version: string | null) => command<InstallPlan>('plan_runtime_install', { recipe, version }),
  log: (id: string, cursor: number, encoding: string) => command<LogPage>('read_run_log', { id, cursor, encoding, limit: 131072 }),
  run: (id: string, overrides: Script | null, secretValues: Record<string, string>, event: (value: RunEvent) => void) => {
    const channel = new Channel<RunEvent>();
    channel.onmessage = event;
    return command<string>('run_script', { id, overrides, secretValues, channel });
  },
};
export async function pickFiles(title: string, extensions: string[], multiple = false): Promise<string[]> {
  if (!desktop) throw new Error('请在桌面客户端中选择本地文件。浏览器预览不会访问文件系统。');
  const result = await open({ title, multiple, filters: [{ name: '文件', extensions }] });
  return result ? (Array.isArray(result) ? result : [result]) : [];
}
export async function pickDirectory(): Promise<string | null> {
  if (!desktop) throw new Error('请在桌面客户端中选择本地目录。');
  return await open({ title: '选择工作目录', directory: true }) as string | null;
}
export async function pickSave(title: string, filename: string, extension: string): Promise<string | null> {
  if (!desktop) throw new Error('请在桌面客户端中导出文件。');
  return save({ title, defaultPath: filename, filters: [{ name: extension.toUpperCase(), extensions: [extension] }] });
}
