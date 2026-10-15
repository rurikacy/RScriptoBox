<script setup lang="ts">
import { computed, defineAsyncComponent, nextTick, onMounted, onUnmounted, ref, toRaw, watch } from 'vue';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
import { ArrowLeft, ArrowRight, ArrowUpRight, Box, Check, CheckCheck, ChevronRight, Clock3, Code2, Copy, Download, ExternalLink, FileCode2, Files, Folder, FolderOpen, HardDrive, Heart, History, Info, LayoutGrid, List, LoaderCircle, MoreHorizontal, Pause, Play, Plus, RefreshCw, Search, Settings2, ShieldCheck, SlidersHorizontal, Square, Terminal, Trash2, TriangleAlert, Upload, X, Zap } from 'lucide-vue-next';
const TerminalView = defineAsyncComponent(() => import('./TerminalView.vue'));
import { api, command, desktop, pickDirectory, pickFiles, pickSave } from './api';
import { activeStates, compatibleRuntime, filterScripts, kindNames, labelState, limitLog, matchesRunState, parseCodes, pendingInstallStates, runEncoding, secretSlots, selectTaskRuns, type AppInfo, type Environment, type InputValue, type InstallJob, type InstallPlan, type Run, type RunEvent, type Runtime, type Script, type Settings } from './model';

type Page = 'library' | 'tasks' | 'environments' | 'history' | 'settings';
const page = ref<Page>('library');
watch(page, async () => { await nextTick(); window.scrollTo({ top: 0, behavior: 'instant' }); });
const navigation = [
  { id: 'library' as Page, name: '脚本库', icon: LayoutGrid },
  { id: 'tasks' as Page, name: '任务与日志', icon: Terminal },
  { id: 'environments' as Page, name: '环境中心', icon: Box },
  { id: 'history' as Page, name: '运行历史', icon: History },
  { id: 'settings' as Page, name: '偏好设置', icon: Settings2 },
];
const pageMeta = computed(() => [navigation.find(nav => nav.id === page.value)!.name]);
const scripts = ref<Script[]>([]);
const runtimes = ref<Runtime[]>([]);
const environments = ref<Environment[]>([]);
const runs = ref<Run[]>([]);
const jobs = ref<InstallJob[]>([]);
const appInfo = ref<AppInfo>({ version: '0.2.0', dataDir: '', portable: false, elevated: false });
const settings = ref<Settings>({ maxConcurrent: 4, retentionDays: 30, logLimitMib: 20, totalLogMib: 500, defaultMode: 'terminal', defaultEncoding: 'system' });
const loading = ref(false);
const busy = ref(false);
const search = ref('');
const typeFilter = ref('');
const tagFilter = ref('');
const favoriteOnly = ref(false);
const fileFilter = ref('');
const viewMode = ref('list');
const savedFont = Number(localStorage.getItem('rscriptobox.fontSize'));
const uiFontSize = ref([14, 16, 18, 20].includes(savedFont) ? savedFont : 16);
watch(uiFontSize, value => { document.documentElement.style.fontSize = `${value}px`; localStorage.setItem('rscriptobox.fontSize', String(value)); }, { immediate: true });
const terminalView = ref<InstanceType<typeof TerminalView>>();
const searchInput = ref<HTMLInputElement>();
const dragOver = ref(false);
const moreMenu = ref<string | null>(null);
const allTags = computed(() => [...new Set(scripts.value.flatMap(s => s.tags))].sort());
const visibleScripts = computed(() => filterScripts(scripts.value, search.value, typeFilter.value, tagFilter.value, favoriteOnly.value, fileFilter.value));
const libraryPage = ref(0);
const libraryPages = computed(() => Math.max(1, Math.ceil(visibleScripts.value.length / 60)));
watch(libraryPages, total => { libraryPage.value = Math.min(libraryPage.value, total - 1); });
const pageScripts = computed(() => visibleScripts.value.slice(libraryPage.value * 60, (libraryPage.value + 1) * 60));
watch([search, typeFilter, tagFilter, favoriteOnly, fileFilter], () => { libraryPage.value = 0; });
const activeRuns = computed(() => runs.value.filter(r => activeStates.has(r.state)));
const visibleRuns = computed(() => selectTaskRuns(runs.value));
const availableRuntimes = computed(() => runtimes.value.filter(r => ['ready', 'verified', 'available'].includes(r.status)));
const favorites = computed(() => scripts.value.filter(s => s.favorite).length);
const todayRuns = computed(() => runs.value.filter(r => new Date(r.startedAt).toLocaleDateString() === new Date().toLocaleDateString()).length);
const notices = ref<{ id: number; message: string; error: boolean }[]>([]);
let noticeId = 0;
function toast(message: string, error = false) {
  const id = ++noticeId;
  notices.value.push({ id, message, error });
  window.setTimeout(() => { notices.value = notices.value.filter(n => n.id !== id); }, error ? 12000 : 5000);
}
function errorMessage(error: unknown) { return error instanceof Error ? error.message : String(error); }
async function perform<T>(action: () => Promise<T>): Promise<T | undefined> {
  try { return await action(); } catch (error) { toast(errorMessage(error), true); }
}
async function refresh() {
  loading.value = true;
  try {
    const result = await Promise.allSettled([api.scripts(), api.runtimes(), api.environments(), api.runs(), api.installs(), api.settings(), api.info()]);
    const setters = [(v: unknown) => scripts.value = v as Script[], (v: unknown) => runtimes.value = v as Runtime[], (v: unknown) => environments.value = v as Environment[], (v: unknown) => runs.value = v as Run[], (v: unknown) => jobs.value = v as InstallJob[], (v: unknown) => settings.value = v as Settings, (v: unknown) => appInfo.value = v as AppInfo];
    result.forEach((item, i) => item.status === 'fulfilled' ? setters[i]!(item.value) : toast(errorMessage(item.reason), true));
  } finally { loading.value = false; }
}
function managedScript(script: Script) { return !desktop || script.path.replaceAll('\\', '/').toLowerCase().startsWith(appInfo.value.dataDir.replaceAll('\\', '/').toLowerCase() + '/scripts/'); }
function runtimeFor(script: Script) { return runtimes.value.find(r => r.id === script.runtimeId); }
function typeIcon(kind: string) { return kind === 'python' ? Code2 : Terminal; }
function runtimeText(script: Script) {
  if (script.environmentId) return 'Python venv';
  const runtime = runtimeFor(script);
  return runtime ? `${kindNames[runtime.kind] || runtime.kind} ${runtime.version}` : '未绑定环境';
}
function isRuntimeReady(script: Script) {
  if (script.environmentId) return environments.value.some(e => e.id === script.environmentId && e.dependencyStatus === 'ready');
  const runtime = runtimeFor(script);
  return !!runtime && ['ready', 'verified', 'available'].includes(runtime.status);
}
function latestRun(scriptId: string) { return runs.value.find(r => r.scriptId === scriptId); }
function formatDate(date: string, full = false) {
  if (!date) return '—';
  return new Date(date).toLocaleString('zh-CN', full ? { year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit' } : { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' });
}
const clock = ref(Date.now());
function duration(run: Run) {
  const seconds = Math.max(0, Math.floor(((run.finishedAt ? new Date(run.finishedAt).getTime() : clock.value) - new Date(run.startedAt).getTime()) / 1000));
  return seconds < 60 ? `${seconds} 秒` : `${Math.floor(seconds / 60)} 分 ${seconds % 60} 秒`;
}
function statusClass(state: string) {
  return ['ready', 'available', 'verified', 'succeeded', 'success', 'clean'].includes(state) ? 'good' : ['failed', 'start_failed', 'spawn_failed', 'unavailable', 'missing'].includes(state) ? 'bad' : activeStates.has(state) || ['installing', 'creating', 'verifying'].includes(state) ? 'working' : 'neutral';
}
const confirmation = ref<{ title: string; message: string; label: string; danger: boolean; action: () => Promise<void> } | null>(null);
function confirm(title: string, message: string, label: string, action: () => Promise<void>, danger = false) { confirmation.value = { title, message, label, action, danger }; }
async function acceptConfirmation() {
  const current = confirmation.value;
  if (!current) return;
  busy.value = true;
  try { await current.action(); confirmation.value = null; } catch (error) { toast(errorMessage(error), true); }
  finally { busy.value = false; }
}
async function addPaths(paths: string[]) {
  if (!paths.length) return;
  const result = await command<{ added: Script[]; errors: string[] }>('add_scripts', { paths });
  scripts.value = await api.scripts();
  if (result.added.length) toast(`已添加 ${result.added.length} 个脚本`);
  result.errors.forEach(error => toast(error, true));
  if (!result.added.length && !result.errors.length) toast('所选脚本已在脚本库中。');
}
async function addScripts() { await perform(async () => addPaths(await pickFiles('添加脚本 · 支持多选', ['cmd', 'bat', 'ps1', 'py'], true))); }
async function toggleFavorite(script: Script) {
  await perform(async () => {
    const updated = await command<Script>('update_script', { script: { ...script, favorite: !script.favorite } });
    scripts.value = scripts.value.map(s => s.id === script.id ? updated : s);
  });
}
function removeScript(script: Script) {
  moreMenu.value = null;
  confirm('从脚本库移除', `确定移除“${script.name}”的管理记录吗？工作台副本、原始文件和运行历史会保留。`, '移除记录', async () => {
    await command('remove_script', { id: script.id });
    scripts.value = scripts.value.filter(s => s.id !== script.id);
    toast('已移除记录，文件保留。');
  }, true);
}
async function relocate(script: Script) {
  moreMenu.value = null;
  await perform(async () => {
    const paths = await pickFiles('选择文件并创建新副本', script.kind === 'python' ? ['py'] : script.kind === 'cmd' ? ['cmd', 'bat'] : ['ps1']);
    if (paths[0]) { await command('relocate_script', { id: script.id, path: paths[0] }); scripts.value = await api.scripts(); toast('已复制新版本，旧副本保留，请复核运行配置。'); }
  });
}
async function scriptAction(action: string, script: Script) { moreMenu.value = null; await perform(() => command(action, { id: script.id })); }

const editing = ref<Script | null>(null);
const editTab = ref('general');
const editTags = ref('');
const editCodes = ref('0');
const envRows = ref<{ key: string; value: InputValue }[]>([]);
const configErrors = ref('');
function openConfig(script: Script) {
  editing.value = JSON.parse(JSON.stringify(toRaw(script)));
  if (editing.value?.runMode === 'terminal') editing.value.encoding = 'utf-8';
  editTags.value = script.tags.join('，');
  editCodes.value = script.successCodes.join(', ');
  envRows.value = Object.entries(script.env).map(([key, value]) => ({ key, value: structuredClone(toRaw(value)) }));
  editTab.value = 'general'; configErrors.value = ''; moreMenu.value = null;
}
function addArgument() { editing.value?.args.push(''); }
function toggleArgumentSecret(index: number) {
  if (!editing.value) return;
  const value = editing.value.args[index];
  editing.value.args[index] = typeof value === 'string' ? { secretSlot: `arg_${index + 1}`, required: true } : '';
}
function toggleEnvSecret(index: number) {
  const row = envRows.value[index]!;
  row.value = typeof row.value === 'string' ? { secretSlot: row.key || `env_${index + 1}`, required: true } : '';
}
const compatibleRuntimes = computed(() => editing.value ? runtimes.value.filter(r => compatibleRuntime(editing.value!.kind, r.kind)) : []);
function changeRuntimeBinding() {
  const script = editing.value;
  if (!script) return;
  if (script.environmentId && environments.value.find(e => e.id === script.environmentId)?.baseRuntimeId !== script.runtimeId) script.environmentId = null;
  const runtime = runtimes.value.find(r => r.id === script.runtimeId);
  const matching = runtime && environments.value.find(e => e.interpreterPath.toLowerCase() === runtime.executablePath.toLowerCase());
  if (matching) script.environmentId = matching.id;
}
function changeEnvironmentBinding() {
  const environment = environments.value.find(e => e.id === editing.value?.environmentId);
  if (editing.value && environment) editing.value.runtimeId = environment.baseRuntimeId;
}
const executionPreview = computed(() => {
  const script = editing.value;
  if (!script) return '';
  const runtime = runtimes.value.find(r => r.id === script.runtimeId);
  const interpreter = environments.value.find(e => e.id === script.environmentId)?.interpreterPath || runtime?.executablePath || '<请选择解释器>';
  return [interpreter, script.path, ...script.args.map(value => typeof value === 'string' ? value : `<敏感输入:${value.secretSlot}>`)].map(value => JSON.stringify(value)).join('  ');
});
function collectConfig(): Script {
  const script = editing.value;
  if (!script) throw new Error('未选择脚本。');
  if (!script.name.trim()) throw new Error('请填写脚本名称。');
  if (script.timeoutSeconds !== null && (!Number.isInteger(script.timeoutSeconds) || script.timeoutSeconds < 1)) throw new Error('超时必须为正整数秒，或留空表示不限。');
  const env: Record<string, InputValue> = {};
  for (const row of envRows.value) {
    if (!row.key.trim() || row.key.includes('=') || /[\r\n\0]/.test(row.key)) throw new Error('环境变量名称不能为空，不能包含等号、换行或空字符。');
    if (Object.keys(env).some(key => key.toLowerCase() === row.key.toLowerCase())) throw new Error(`环境变量名称重复：${row.key}`);
    env[row.key] = row.value;
  }
  for (const value of [...script.args, ...Object.values(env)]) {
    if (typeof value !== 'string' && !value.secretSlot.trim()) throw new Error('请为敏感输入填写槽位名称。');
  }
  const slotNames = [...script.args, ...Object.values(env)].filter((value): value is Exclude<InputValue, string> => typeof value !== 'string').map(value => value.secretSlot);
  if (new Set(slotNames).size !== slotNames.length) throw new Error('敏感槽位名称不能重复，请为每个输入使用独立名称。');
  return { ...JSON.parse(JSON.stringify(script)), name: script.name.trim(), cwd: script.cwd.trim(), env, tags: [...new Set(editTags.value.split(/[,，]/).map(t => t.trim()).filter(Boolean))], successCodes: parseCodes(editCodes.value), timeoutSeconds: script.timeoutSeconds || null };
}
async function saveConfig() {
  busy.value = true;
  try {
    const script = await command<Script>('update_script', { script: collectConfig() });
    scripts.value = scripts.value.map(s => s.id === script.id ? script : s);
    editing.value = null; toast('执行配置已保存。');
  } catch (error) { configErrors.value = errorMessage(error); toast(configErrors.value, true); }
  finally { busy.value = false; }
}
const pendingRun = ref<{ script: Script; overrides: Script | null } | null>(null);
const secretValues = ref<Record<string, string>>({});
const currentSecretSlots = computed(() => pendingRun.value ? secretSlots(pendingRun.value.script) : []);
async function requestRun(script: Script, overrides: Script | null = null) {
  if (!desktop) { toast('当前为浏览器界面预览。脚本执行需使用桌面客户端。', true); return; }
  if (!managedScript(script)) { toast('旧记录尚未复制到工作台，请重新导入副本后运行。', true); return; }
  if (!script.exists) { toast('脚本文件不存在，请重新定位后运行。', true); return; }
  if (!script.runtimeId && !script.environmentId) { openConfig(script); editTab.value = 'execution'; toast('请先绑定明确的运行环境。'); return; }
  if (activeRuns.value.some(r => r.scriptId === script.id)) { toast('此脚本正在运行，结束后可再次启动。', true); return; }
  if (secretSlots(script).length) { secretValues.value = {}; pendingRun.value = { script, overrides }; }
  else await executeRun(script, overrides, {});
}
async function runTemporary() {
  try { const script = collectConfig(); await requestRun(script, script); }
  catch (error) { configErrors.value = errorMessage(error); toast(configErrors.value, true); }
}
async function executeRun(script: Script, overrides: Script | null, secrets: Record<string, string>) {
  busy.value = true;
  await perform(async () => {
    const id = await api.run(script.id, overrides, secrets, receiveRunEvent);
    pendingRun.value = null; secretValues.value = {}; editing.value = null;
    runs.value = await api.runs(); page.value = 'tasks';
    await openLog(id, false);
    toast('任务已启动。');
  });
  secretValues.value = {}; busy.value = false;
}
async function submitSecrets() {
  if (!pendingRun.value) return;
  if (currentSecretSlots.value.some(s => s.required && !(secretValues.value[s.name] || '').length)) { toast('请填写所有必填的敏感输入。', true); return; }
  const { script, overrides } = pendingRun.value;
  await executeRun(script, overrides, { ...secretValues.value });
}
function stopRun(run: Run) {
  confirm('强制停止任务', `将强制结束“${run.scriptName}”的受控进程树。任务正在写入的文件可能只完成一部分。`, '强制停止', async () => {
    await command('stop_run', { id: run.id }); runs.value = await api.runs();
  }, true);
}

const selectedRunId = ref('');
const selectedRun = computed(() => runs.value.find(r => r.id === selectedRunId.value));
const runLog = ref('');
const logEncoding = ref('utf-8');
const logCursor = ref(0);
const logHasMore = ref(false);
const logBusy = ref(false);
const logReplay = ref(false);
const autoScroll = ref(true);
const logView = ref<HTMLElement>();
const liveLogs = new Map<string, string>();
const pendingAcks = new Map<string, number>();
let logGeneration = 0;
function scrollLog() { if (autoScroll.value) nextTick(() => { if (logView.value) logView.value.scrollTop = logView.value.scrollHeight; }); }
function receiveRunEvent(event: RunEvent) {
  if (event.type === 'output') pendingAcks.set(event.runId, Math.max(pendingAcks.get(event.runId) || 0, event.seq));
  if (event.type === 'output' || event.type === 'gap' || event.type === 'log_truncated') {
    const text = event.type === 'output' ? (event.stream === 'stderr' ? `[stderr] ${event.text}` : event.text) : event.type === 'gap' ? '\n[实时显示有跳过，可重新读取已保存日志]\n' : '\n[日志已达到配额，后续输出不再保存]\n';
    const bounded = limitLog((liveLogs.get(event.runId) || '') + text);
    liveLogs.set(event.runId, bounded);
    if (selectedRunId.value === event.runId && !logReplay.value) { runLog.value = limitLog(runLog.value + text); scrollLog(); }
  } else {
    const run = runs.value.find(r => r.id === event.runId);
    if (run) { run.state = event.state; if (event.type === 'state') run.reason = event.reason || null; else run.rootExitCode = event.rootExitCode ?? null; }
    if (event.type === 'finished') { liveLogs.delete(event.runId); void perform(async () => { runs.value = await api.runs(); scripts.value = await api.scripts(); }); }
  }
}
async function openLog(id: string, fromDisk = false) {
  const sameRun = selectedRunId.value === id;
  selectedRunId.value = id;
  const run = runs.value.find(r => r.id === id);
  if (!sameRun) logEncoding.value = runEncoding(run);
  if (run?.runMode === 'console') { runLog.value = ''; return; }
  const generation = ++logGeneration;
  if (!fromDisk && run && activeStates.has(run.state)) {
    logReplay.value = false; logEncoding.value = runEncoding(run); logBusy.value = false;
    runLog.value = liveLogs.get(id) || ''; logCursor.value = 0; logHasMore.value = false; scrollLog(); return;
  }
  logReplay.value = true;
  runLog.value = ''; logCursor.value = 0; logHasMore.value = false; logBusy.value = true;
  try {
    const result = await api.log(id, 0, logEncoding.value);
    if (generation !== logGeneration) return;
    runLog.value = limitLog(result.text); logCursor.value = result.nextCursor; logHasMore.value = result.hasMore;
    scrollLog();
  } catch (error) { toast(errorMessage(error), true); }
  finally { if (generation === logGeneration) logBusy.value = false; }
}
async function nextLogPage() {
  const generation = logGeneration;
  logBusy.value = true;
  await perform(async () => {
    const id = selectedRunId.value;
    const result = await api.log(id, logCursor.value, logEncoding.value);
    if (id !== selectedRunId.value || generation !== logGeneration) return;
    runLog.value = limitLog(runLog.value + result.text); logCursor.value = result.nextCursor; logHasMore.value = result.hasMore; scrollLog();
  });
  if (generation === logGeneration) logBusy.value = false;
}
async function copyLog() { await perform(async () => { await navigator.clipboard.writeText(selectedRun.value?.runMode === 'terminal' ? terminalView.value?.plainText() || '' : runLog.value); toast('当前显示的日志已复制。'); }); }
function exportLog(run: Run) {
  confirm('导出日志', '脚本主动输出的密码、令牌或其他敏感内容可能存在于日志中。请在分享导出文件前检查。', '选择保存位置', async () => {
    const path = await pickSave('导出完整日志', `${run.scriptName}-${run.id.slice(0, 8)}.txt`, 'txt');
    if (path) { await command('export_run_log', { id: run.id, path, encoding: logEncoding.value }); toast('日志已导出。'); }
  });
}
const historyQuery = ref('');
const historyState = ref('');
const historyDays = ref('');
const historyRuns = computed(() => runs.value.filter(r => !activeStates.has(r.state) && (!historyQuery.value || `${r.scriptName} ${r.scriptPath}`.toLowerCase().includes(historyQuery.value.toLowerCase())) && matchesRunState(r.state, historyState.value) && (!historyDays.value || Date.now() - new Date(r.startedAt).getTime() <= Number(historyDays.value) * 86400000)));
function viewHistory(run: Run) { page.value = 'tasks'; void openLog(run.id); }

const scanning = ref(false);
async function detectEnvironments() {
  scanning.value = true;
  await perform(async () => { runtimes.value = await command<Runtime[]>('refresh_runtimes'); environments.value = await api.environments(); toast('环境检测完成。'); });
  scanning.value = false;
}
const registerDialog = ref(false);
const registerKind = ref('python');
const registerPath = ref('');
async function chooseRuntimePath() { await perform(async () => { const paths = await pickFiles('选择实际的解释器程序', ['exe']); if (paths[0]) registerPath.value = paths[0]; }); }
async function registerRuntime() {
  busy.value = true;
  await perform(async () => {
    await command<Runtime>('register_runtime', { kind: registerKind.value, path: registerPath.value });
    [runtimes.value, environments.value] = await Promise.all([api.runtimes(), api.environments()]); registerDialog.value = false; registerPath.value = ''; toast('解释器已验证并登记。');
  });
  busy.value = false;
}
const installPlan = ref<InstallPlan | null>(null);
const installAccepted = ref(false);
const installVersion = ref('3.14');
async function planInstall(recipe: string) {
  busy.value = true;
  await perform(async () => { installPlan.value = await api.plan(recipe, recipe === 'python' ? installVersion.value : null); installAccepted.value = false; });
  busy.value = false;
}
async function startInstall() {
  if (!installPlan.value || !installAccepted.value) return;
  busy.value = true;
  await perform(async () => {
    await command<InstallJob>('start_runtime_install', { planId: installPlan.value!.id });
    installPlan.value = null; jobs.value = await api.installs(); toast('安装已开始，可在下方查看真实进度与日志。');
  });
  busy.value = false;
}
const selectedJob = ref<InstallJob | null>(null);
const installLog = ref('');
async function viewInstallLog(job: InstallJob) { selectedJob.value = job; await perform(async () => { installLog.value = limitLog(await command<string>('read_install_log', { id: job.id })); }); }
async function recheckJob(job: InstallJob) {
  confirm('确认安装器状态并复检', '请确认相关安装窗口和系统安装器已经结束。复检会重新检查解释器与依赖；无法确认结果时会继续保留待复检状态。', '安装器已结束，开始复检', async () => {
    await command('recheck_install', { id: job.id, installerClosed: true });
    await refresh(); toast('已完成复检，请查看环境和安装状态。');
  });
}
function cancelInstall(job: InstallJob) {
  confirm('请求取消安装', '外部安装器可能继续运行，也不会自动回滚已安装的组件。操作后请等待安装器结束并复检。', '请求取消', async () => {
    await command('cancel_runtime_install', { id: job.id }); jobs.value = await api.installs();
  });
}
const venvDialog = ref(false);
const venvScript = ref('');
const venvRuntime = ref('');
const pythonScripts = computed(() => scripts.value.filter(s => s.kind === 'python'));
const pythonRuntimes = computed(() => availableRuntimes.value.filter(r => r.kind === 'python'));
async function createVenv() {
  busy.value = true;
  await perform(async () => {
    await command('create_python_environment', { scriptId: venvScript.value, baseRuntimeId: venvRuntime.value });
    venvDialog.value = false; jobs.value = await api.installs(); toast('正在创建专用环境，验证成功后会绑定到脚本。');
  });
  busy.value = false;
}
async function installRequirements(environment: Environment) {
  await perform(async () => {
    const paths = await pickFiles('选择要安装的 requirements.txt', ['txt']);
    if (!paths[0]) return;
    const path = paths[0];
    confirm('安装 Python 依赖', `目标环境：${environment.interpreterPath}\n依赖清单：${path}\n\n此操作会下载软件包并可能执行第三方构建代码。安装期间该环境将被锁定。`, '确认安装依赖', async () => {
      await command('install_requirements', { environmentId: environment.id, path }); jobs.value = await api.installs(); toast('依赖安装任务已开始。');
    });
  });
}
async function saveSettings() {
  busy.value = true;
  await perform(async () => { settings.value = await command<Settings>('update_settings', { settings: toRaw(settings.value) }); toast('偏好设置已保存。'); });
  busy.value = false;
}
async function backupDatabase() {
  await perform(async () => { const path = await pickSave('备份 RScriptoBox 数据', `RScriptoBox-${new Date().toISOString().slice(0, 10)}.sqlite`, 'sqlite'); if (path) { await command('backup_database', { path }); toast('数据库备份已保存。'); } });
}
async function restoreDatabase() {
  await perform(async () => {
    const paths = await pickFiles('选择数据库备份', ['sqlite', 'db']);
    if (paths[0]) confirm('恢复数据库', '恢复会替换当前配置与历史记录。请先备份当前数据、停止活动任务，并完成未决安装的复检。源脚本和系统环境不会包含在备份中。', '恢复备份', async () => { await command('restore_database', { path: paths[0] }); await refresh(); toast('数据库已恢复。'); }, true);
  });
}
async function official(product: string) { await perform(() => command('open_official_page', { product })); }
function closeOverlays() { moreMenu.value = null; if (!busy.value) { editing.value = null; registerDialog.value = false; installPlan.value = null; venvDialog.value = false; selectedJob.value = null; pendingRun.value = null; secretValues.value = {}; confirmation.value = null; } }
function keyboard(event: KeyboardEvent) {
  if ((event.target as HTMLElement)?.closest('.interactive-terminal')) return;
  if (event.key === 'Escape') closeOverlays();
  if (!hasOverlay.value && (event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k') { event.preventDefault(); page.value = 'library'; void nextTick(() => searchInput.value?.focus()); }
  if (!hasOverlay.value && (event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'o') { event.preventDefault(); void addScripts(); }
  if (event.key === 'Tab' && hasOverlay.value) {
    const overlay = document.querySelector('.confirm-overlay') || document.querySelector('.modal-overlay') || document.querySelector('.drawer-overlay');
    const elements = [...(overlay?.querySelectorAll<HTMLElement>('button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex="0"]') || [])].filter(el => el.offsetParent !== null);
    const first = elements[0]; const last = elements.at(-1);
    if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
    else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
  }
}
const hasOverlay = computed(() => !!(editing.value || registerDialog.value || installPlan.value || venvDialog.value || selectedJob.value || pendingRun.value || confirmation.value));
watch([editing, registerDialog, installPlan, venvDialog, selectedJob, pendingRun, confirmation], async () => {
  if (hasOverlay.value) { await nextTick(); const overlay = document.querySelector('.confirm-overlay') || document.querySelector('.modal-overlay') || document.querySelector('.drawer-overlay'); overlay?.querySelector<HTMLElement>('input, select, button')?.focus(); }
});
let polling: ReturnType<typeof setInterval>;
let clockTick: ReturnType<typeof setInterval>;
let ackTick: ReturnType<typeof setInterval>;
let unlistenDrop: (() => void) | undefined;
let pollingBusy = false;
let pollCount = 0;
async function poll() {
  if (pollingBusy || !desktop) return;
  pollingBusy = true;
  try {
    const [newRuns, newJobs] = await Promise.all([api.runs(), api.installs()]);
    const jobsChanged = JSON.stringify(newJobs) !== JSON.stringify(jobs.value);
    runs.value = newRuns; jobs.value = newJobs;
    if (jobsChanged || ++pollCount % 5 === 0) {
      [runtimes.value, environments.value, scripts.value] = await Promise.all([api.runtimes(), api.environments(), api.scripts()]);
    }
    if (selectedJob.value) { selectedJob.value = newJobs.find(j => j.id === selectedJob.value?.id) || selectedJob.value; installLog.value = limitLog(await command<string>('read_install_log', { id: selectedJob.value.id })); }
    if (logReplay.value && selectedRun.value && activeStates.has(selectedRun.value.state) && !logBusy.value) await nextLogPage();
  } catch (error) { console.error('刷新任务状态失败', error); }
  finally { pollingBusy = false; }
}
onMounted(async () => {
  document.addEventListener('keydown', keyboard);
  await refresh();
  clockTick = setInterval(() => { clock.value = Date.now(); }, 1000);
  if (desktop) {
    polling = setInterval(poll, 1200);
    ackTick = setInterval(() => {
      for (const [id, seq] of pendingAcks) {
        pendingAcks.delete(id);
        void command('ack_run_output', { id, seq }).catch(error => console.error('确认实时输出失败', error));
      }
    }, 100);
    await perform(async () => {
      unlistenDrop = await getCurrentWebviewWindow().onDragDropEvent(event => {
        dragOver.value = event.payload.type === 'over' || event.payload.type === 'enter';
        if (event.payload.type === 'drop') { const paths = event.payload.paths; dragOver.value = false; void perform(() => addPaths(paths)); }
      });
    });
  }
});
onUnmounted(() => { clearInterval(polling); clearInterval(clockTick); clearInterval(ackTick); unlistenDrop?.(); document.removeEventListener('keydown', keyboard); });
</script>

<template>
  <div class="app-shell" @click="moreMenu && (moreMenu = null)">
    <aside class="sidebar" :inert="hasOverlay">
      <a class="brand" href="#" @click.prevent="page = 'library'" aria-label="RScriptoBox 首页"><span class="brand-icon"><Terminal :size="23" :stroke-width="2.5" /></span><span>RScripto<span class="brand-light">Box</span></span></a>
      <div class="nav-caption">工作空间</div>
      <nav class="nav-list" aria-label="主导航"><button v-for="nav in navigation" :key="nav.id" :aria-label="nav.name" :title="nav.name" :class="['nav-button', { active: page === nav.id }]" @click="page = nav.id"><component :is="nav.icon" :size="19" /><span>{{ nav.name }}</span><span v-if="nav.id === 'library'" class="nav-count">{{ scripts.length }}</span><span v-if="nav.id === 'tasks' && activeRuns.length" class="nav-count running-count">{{ activeRuns.length }}</span></button></nav>
      <div class="sidebar-divider"></div><div class="nav-caption">快捷访问</div>
      <button aria-label="我的收藏" title="我的收藏" :class="['nav-button', 'favorite-nav', { 'sub-active': favoriteOnly && page === 'library' }]" @click="page = 'library'; favoriteOnly = !favoriteOnly"><Heart :size="18" /><span>我的收藏</span><span class="nav-count">{{ favorites }}</span></button>
      <div class="sidebar-bottom"><div class="sidebar-footer"><span class="mini-logo"><Terminal :size="14" /></span><span>RScriptoBox <small>v{{ appInfo.version }}</small></span><span class="platform-label">WIN</span></div></div>
    </aside>
    <main class="main-area" :inert="hasOverlay">
      <header class="topbar"><div class="breadcrumb"><span>工作空间</span><ChevronRight :size="14" /><strong>{{ pageMeta[0] }}</strong></div><div class="topbar-actions"><span v-if="appInfo.portable" class="small-badge"><HardDrive :size="12" />便携模式</span><span v-if="appInfo.elevated" class="elevated-badge"><TriangleAlert :size="13" />当前为管理员权限</span><span class="topbar-separator"></span><button class="icon-button" title="刷新工作台" aria-label="刷新工作台" @click="refresh"><RefreshCw :size="17" :class="{ spin: loading }" /></button></div></header>
      <div v-if="!desktop" class="preview-banner"><Info :size="15" /><span>浏览器界面预览 · 当前展示示例脚本，添加、执行、安装等桌面功能需使用客户端。</span><span class="preview-tag">PREVIEW</span></div>
      <div class="page-content">
        <section class="page-heading"><div><h1>{{ pageMeta[0] }}<span v-if="page === 'library'" class="heading-count">{{ scripts.length }}</span></h1></div><button v-if="page === 'library'" class="button primary" @click="addScripts"><Plus :size="18" />添加脚本<span class="shortcut-light">Ctrl O</span></button><div v-if="page === 'environments'" class="button-row"><button class="button" @click="registerDialog = true"><Plus :size="16" />手动登记</button><button class="button primary" :disabled="scanning" @click="detectEnvironments"><RefreshCw :size="16" :class="{ spin: scanning }" />{{ scanning ? '正在检测' : '重新检测' }}</button></div></section>

        <template v-if="page === 'library'">
          <section class="overview-grid" aria-label="工作台概览"><div class="overview-card"><span class="overview-icon lilac"><Files :size="21" /></span><div><span>全部脚本</span><strong>{{ scripts.length }}</strong></div><span class="overview-decoration">↗</span></div><div class="overview-card"><span class="overview-icon peach"><Heart :size="21" /></span><div><span>我的收藏</span><strong>{{ favorites }}</strong></div></div><div class="overview-card"><span class="overview-icon mint"><Box :size="21" /></span><div><span>可用环境</span><strong>{{ availableRuntimes.length }}</strong></div><span class="dot good-dot"></span></div><div class="overview-card"><span class="overview-icon sky"><Zap :size="21" /></span><div><span>今日运行</span><strong>{{ todayRuns }}</strong></div></div></section>
          <section class="library-panel">
            <div class="library-toolbar"><div class="type-tabs" aria-label="按脚本类型筛选"><button v-for="type in [{ id: '', name: '全部脚本' }, { id: 'python', name: 'Python' }, { id: 'powershell', name: 'PowerShell' }, { id: 'cmd', name: 'CMD / BAT' }]" :key="type.id" :class="{ selected: typeFilter === type.id }" @click="typeFilter = type.id">{{ type.name }}<span v-if="!type.id">{{ scripts.length }}</span></button></div><div class="view-toggle"><button :class="{ selected: viewMode === 'grid' }" aria-label="卡片视图" title="卡片视图" @click="viewMode = 'grid'"><LayoutGrid :size="16" /></button><button :class="{ selected: viewMode === 'list' }" aria-label="列表视图" title="列表视图" @click="viewMode = 'list'"><List :size="17" /></button></div></div>
            <div class="filter-toolbar"><div class="search-field"><Search :size="17" /><input ref="searchInput" v-model="search" placeholder="搜索脚本名称、描述、路径或标签…" aria-label="搜索脚本" /><kbd>Ctrl K</kbd></div><select v-model="tagFilter" aria-label="按标签筛选"><option value="">所有标签</option><option v-for="tag in allTags" :key="tag">{{ tag }}</option></select><select v-model="fileFilter" aria-label="按文件状态筛选"><option value="">所有文件状态</option><option value="exists">文件存在</option><option value="missing">文件不存在</option></select><button :class="['filter-favorite', { selected: favoriteOnly }]" @click="favoriteOnly = !favoriteOnly"><Heart :size="15" :fill="favoriteOnly ? 'currentColor' : 'none'" />仅收藏</button></div>
            <div class="result-line"><span>{{ favoriteOnly ? '我的收藏' : '脚本集合' }}<b>{{ visibleScripts.length }}</b></span></div>
            <div v-if="visibleScripts.length" :class="['script-grid', { 'list-view': viewMode === 'list' }]">
              <article v-for="script in pageScripts" :key="script.id" :class="['script-card', { 'missing-card': !script.exists }]">
                <div class="card-top"><span :class="['script-type-icon', script.kind]"><component :is="typeIcon(script.kind)" :size="24" :stroke-width="1.8" /></span><span :class="['type-label', script.kind]">{{ script.kind === 'cmd' ? (script.path.toLowerCase().endsWith('.bat') ? '.BAT' : '.CMD') : script.kind === 'python' ? '.PY' : '.PS1' }}</span><div class="card-top-actions"><button :class="['icon-button', 'heart-button', { loved: script.favorite }]" :title="script.favorite ? '取消收藏' : '收藏脚本'" :aria-label="script.favorite ? '取消收藏' : '收藏脚本'" @click="toggleFavorite(script)"><Heart :size="16" :fill="script.favorite ? 'currentColor' : 'none'" /></button><div class="menu-anchor"><button class="icon-button" aria-label="更多操作" title="更多操作" @click.stop="moreMenu = moreMenu === script.id ? null : script.id"><MoreHorizontal :size="19" /></button><div v-if="moreMenu === script.id" class="dropdown-menu" @click.stop><button @click="openConfig(script)"><SlidersHorizontal :size="15" />执行配置</button><button @click="scriptAction('open_script_location', script)"><FolderOpen :size="15" />打开所在目录</button><button @click="scriptAction('edit_script', script)"><FileCode2 :size="15" />使用系统编辑器</button><button @click="relocate(script)"><Folder :size="15" />重新导入副本</button><div class="menu-divider"></div><button class="danger-text" @click="removeScript(script)"><Trash2 :size="15" />移除管理记录</button></div></div></div></div>
                <div class="card-description"><button class="script-title" @click="openConfig(script)">{{ script.name }}</button><p v-if="script.description">{{ script.description }}</p></div>
                <div class="tag-row"><span v-for="tag in script.tags.slice(0, 3)" :key="tag" class="tag" @click="tagFilter = tag">{{ tag }}</span></div>
                <div class="script-file" :title="script.path"><FileCode2 :size="12" /><span>{{ script.path.split(/[\\/]/).pop() }}</span></div>
                <div class="card-environment"><span :class="['dot', !script.exists ? 'bad-dot' : isRuntimeReady(script) ? 'good-dot' : 'warning-dot']"></span><span>{{ !script.exists ? '副本不存在 · 请重新导入' : !managedScript(script) ? '尚未复制 · 请重新导入' : runtimeText(script) }}</span><span v-if="latestRun(script.id)" class="last-run">{{ labelState(latestRun(script.id)!.state) }}</span></div>
                <div class="card-footer"><button class="config-button" @click="openConfig(script)"><SlidersHorizontal :size="14" />配置</button><button :class="['run-button', { disabled: !script.exists }]" :disabled="!script.exists || activeRuns.some(r => r.scriptId === script.id)" @click="requestRun(script)"><LoaderCircle v-if="activeRuns.some(r => r.scriptId === script.id)" :size="14" class="spin" /><Play v-else :size="13" :fill="script.exists ? 'currentColor' : 'none'" />{{ activeRuns.some(r => r.scriptId === script.id) ? '运行中' : '运行脚本' }}</button></div>
              </article>
            </div>
            <div v-else class="empty-state"><span class="empty-icon"><Search v-if="scripts.length" :size="30" /><Files v-else :size="30" /></span><h3>{{ scripts.length ? '没有找到匹配的脚本' : '尚未添加脚本' }}</h3><p>{{ scripts.length ? '试试其他关键词，或调整类型、标签与文件状态。' : '添加 .py、.ps1、.cmd 或 .bat 文件，集中管理，轻松运行。' }}</p><button v-if="scripts.length" class="button" @click="search = ''; typeFilter = ''; tagFilter = ''; fileFilter = ''; favoriteOnly = false">清除筛选</button><button v-else class="button primary" @click="addScripts"><Plus :size="16" />添加脚本</button></div>
            <div v-if="libraryPages > 1" class="pagination"><span>每页 60 个脚本 · 第 {{ libraryPage + 1 }} / {{ libraryPages }} 页</span><button class="button small" :disabled="libraryPage === 0" @click="libraryPage--"><ArrowLeft :size="13" />上一页</button><button class="button small" :disabled="libraryPage + 1 >= libraryPages" @click="libraryPage++">下一页<ArrowRight :size="13" /></button></div><button class="drop-zone" @click="addScripts"><span class="drop-icon"><Upload :size="18" /></span><span><strong>把脚本拖到这里，或点击添加</strong><small>导入到 data/scripts · 后续运行和编辑工作台副本</small></span><span class="drop-plus"><Plus :size="20" /></span></button>
          </section>
          
        </template>

        <template v-else-if="page === 'tasks'">
          <div class="tasks-layout"><aside class="task-list-panel"><div class="panel-label"><strong>执行任务</strong><span class="count-pill">{{ activeRuns.length }} 运行中</span></div><button v-for="run in visibleRuns" :key="run.id" :class="['task-item', { selected: selectedRunId === run.id }]" @click="openLog(run.id)"><span :class="['task-state-icon', statusClass(run.state)]"><LoaderCircle v-if="activeStates.has(run.state)" :size="16" class="spin" /><Check v-else-if="['succeeded', 'success'].includes(run.state)" :size="16" /><Square v-else :size="14" /></span><span><strong>{{ run.scriptName }}</strong><small>{{ formatDate(run.startedAt) }} · {{ duration(run) }}</small></span><span :class="['state-label', statusClass(run.state)]">{{ labelState(run.state) }}</span></button><div v-if="!runs.length" class="compact-empty"><Terminal :size="28" /><p>这里还没有任务</p><small>从脚本库运行一个脚本吧。</small></div></aside><section class="log-panel"><template v-if="selectedRun"><div class="log-heading"><div><h3>{{ selectedRun.scriptName }}</h3><span>{{ selectedRun.runtimePath }}</span></div><button v-if="activeStates.has(selectedRun.state)" class="button danger-outline" @click="stopRun(selectedRun)"><Square :size="13" fill="currentColor" />强制停止</button><span v-else :class="['status-badge', statusClass(selectedRun.state)]">{{ labelState(selectedRun.state) }}</span></div><div class="run-metadata"><span><Clock3 :size="13" />{{ duration(selectedRun) }}</span><span>退出码 <b>{{ selectedRun.rootExitCode ?? '—' }}</b></span><span>{{ selectedRun.runMode === 'console' ? '系统控制台' : selectedRun.runMode === 'terminal' ? '内置终端' : '旧版日志' }}</span><span v-if="selectedRun.logTruncated" class="warning-text">日志已截断</span></div><div v-if="selectedRun.reason" class="inline-warning"><TriangleAlert :size="15" />{{ selectedRun.reason }}</div><template v-if="selectedRun.runMode !== 'console'"><div class="log-toolbar"><select v-model="logEncoding" :disabled="selectedRun.runMode === 'terminal'" aria-label="日志编码" @change="openLog(selectedRunId, true)"><option value="utf-8">UTF-8</option><option value="system">系统代码页</option><option value="gb18030">GB18030</option></select><button class="text-button" @click="openLog(selectedRunId, true)"><RefreshCw :size="14" />重新读取</button><button v-if="logReplay && activeStates.has(selectedRun.state)" class="text-button" @click="openLog(selectedRunId)"><Zap :size="13" />返回实时</button><div class="spacer"></div><button :class="['icon-button', { active: !autoScroll }]" :title="autoScroll ? '暂停自动滚动（继续采集）' : '恢复自动滚动'" :aria-label="autoScroll ? '暂停自动滚动' : '恢复自动滚动'" @click="autoScroll = !autoScroll; scrollLog()"><Pause v-if="autoScroll" :size="16" /><Play v-else :size="16" /></button><button class="icon-button" title="复制当前显示日志" aria-label="复制日志" @click="copyLog"><Copy :size="15" /></button><button class="icon-button" title="导出完整日志" aria-label="导出日志" @click="exportLog(selectedRun)"><Download :size="16" /></button></div><TerminalView v-if="selectedRun.runMode === 'terminal'" :key="selectedRun.id" ref="terminalView" :run-id="selectedRun.id" :text="runLog" :interactive="activeStates.has(selectedRun.state) && !logReplay" :font-size="uiFontSize" :auto-scroll="autoScroll" @error="toast($event, true)" /><div v-else ref="logView" class="terminal-output" tabindex="0"><pre v-if="runLog">{{ runLog }}</pre><div v-else class="terminal-placeholder"><LoaderCircle v-if="logBusy" :size="20" class="spin" /><Terminal v-else :size="24" /><span>{{ logBusy ? '正在读取日志…' : activeStates.has(selectedRun.state) ? '等待脚本输出…' : '此任务没有保存的输出。' }}</span></div></div><div class="log-status"><span><span :class="['dot', activeStates.has(selectedRun.state) ? 'good-dot' : 'muted-dot']"></span>{{ logReplay ? '只读回放' : selectedRun.runMode === 'terminal' && activeStates.has(selectedRun.state) ? '点击终端输入 · Ctrl+C 中断' : autoScroll ? '自动滚动' : '滚动暂停' }}</span><button v-if="logHasMore" class="text-button" :disabled="logBusy" @click="nextLogPage">读取后续日志<ChevronRight :size="13" /></button><span v-else>最多显示最近 10,000 行</span></div></template><div v-else class="empty-state console-empty"><Terminal :size="38" /><h3>此任务在系统控制台中运行</h3><p>请在独立控制台窗口查看输出和进行交互。运行状态和退出码会保存在这里。</p></div></template><div v-else class="empty-state"><span class="empty-icon"><Terminal :size="30" /></span><h3>选择一个任务，查看执行详情</h3><p>实时输出、运行结果和历史日志都会呈现在这里。</p><button class="button primary" @click="page = 'library'"><LayoutGrid :size="15" />前往脚本库</button></div></section></div>
        </template>

        <template v-else-if="page === 'environments'">
          <div class="info-strip"><ShieldCheck :size="19" /><div><strong>绑定明确的解释器，让每次执行都可预期</strong><p>检测和安装不会自动运行脚本。CMD 与 Windows PowerShell 为系统组件，仅检测、不替换。</p></div></div>
          <section class="section-block"><div class="section-title"><h2>运行环境<span>{{ runtimes.length }}</span></h2><span>每个解释器独立验证与管理</span></div><div class="runtime-grid"><article v-for="runtime in runtimes" :key="runtime.id" class="runtime-card"><div class="runtime-top"><span :class="['script-type-icon', runtime.kind === 'windows_powershell' ? 'powershell' : runtime.kind]"><component :is="typeIcon(runtime.kind)" :size="25" /></span><span :class="['status-badge', statusClass(runtime.status)]"><span class="dot"></span>{{ labelState(runtime.status) }}</span></div><h3>{{ kindNames[runtime.kind] || runtime.kind }}<span>{{ runtime.version }}</span></h3><p class="runtime-path" :title="runtime.executablePath">{{ runtime.executablePath }}</p><div class="runtime-bottom"><span>{{ runtime.architecture }}<i>·</i>{{ runtime.source === 'system' ? '系统内置' : runtime.source === 'manual' ? '手动登记' : '自动发现' }}</span><span v-if="runtime.hasPip" class="tiny-badge">pip 可用</span></div><p v-if="runtime.executionPolicy" class="runtime-policy">执行策略：{{ runtime.executionPolicy }}</p><div class="runtime-bindings">{{ scripts.filter(s => s.runtimeId === runtime.id).length }} 个脚本绑定此环境</div></article><button class="runtime-add" @click="registerDialog = true"><span><Plus :size="26" /></span><strong>添加已有环境</strong><small>选择 python.exe、pwsh.exe 等实际解释器</small></button></div></section>
          <section class="section-block"><div class="section-title"><h2>准备新的环境</h2><span>由官方来源安装，完成后自动复检</span></div><div class="install-options"><article class="install-option"><span class="script-type-icon python"><Code2 :size="25" /></span><div><h3>Python<span class="tiny-badge">官方运行时</span></h3><p>通过 Python Install Manager 管理器安装指定版本。</p></div><select v-model="installVersion" aria-label="Python 版本"><option value="3.14">Python 3.14</option><option value="3.13">Python 3.13</option><option value="3.12">Python 3.12</option></select><button class="button" :disabled="busy" @click="planInstall('python')"><Download :size="15" />查看安装计划</button></article><article class="install-option"><span class="script-type-icon powershell"><Terminal :size="25" /></span><div><h3>PowerShell 7</h3><p>现代 PowerShell，与系统内置 5.1 并存。</p></div><button class="button" :disabled="busy" @click="planInstall('powershell')"><Download :size="15" />查看安装计划</button></article></div><div class="official-links"><Info :size="14" /><span>没有 WinGet 或受企业策略限制？</span><button @click="official('python')">Python 官方下载<ArrowUpRight :size="12" /></button><button @click="official('powershell')">PowerShell 官方说明<ArrowUpRight :size="12" /></button><button @click="official('winget')">获取 WinGet<ArrowUpRight :size="12" /></button></div></section>
          <section class="section-block"><div class="section-title"><h2>Python 专用环境<span>{{ environments.length }}</span></h2><button class="button small" @click="venvDialog = true"><Plus :size="14" />创建 venv</button></div><div v-if="!environments.length" class="empty-inline"><Box :size="23" /><div><strong>为需要依赖的脚本，创建一个独立空间</strong><p>venv 与系统 Python 隔离，依赖只安装到指定环境中。</p></div><button class="text-button" @click="venvDialog = true">创建专用环境<ArrowRight :size="14" /></button></div><div v-else class="environment-list"><article v-for="environment in environments" :key="environment.id" class="environment-row"><span class="overview-icon lilac"><Box :size="20" /></span><div><strong>{{ scripts.find(s => s.environmentId === environment.id)?.name || '未绑定的专用环境' }}</strong><p>{{ environment.interpreterPath }}</p><small v-if="environment.requirementsPath">依赖清单：{{ environment.requirementsPath }}</small></div><span :class="['status-badge', statusClass(environment.dependencyStatus)]">{{ labelState(environment.dependencyStatus) }}</span><button class="button small" @click="installRequirements(environment)"><Download :size="14" />安装依赖</button></article></div></section>
          <section v-if="jobs.length" class="section-block"><div class="section-title"><h2>安装与依赖任务<span>{{ jobs.length }}</span></h2><span>外部安装器启动后可能需要 UAC 或系统交互</span></div><div class="install-jobs"><article v-for="job in jobs" :key="job.id" class="install-job"><div class="install-job-main"><LoaderCircle v-if="['installing', 'verifying', 'creating'].includes(job.state)" :size="20" class="spin purple-text" /><Box v-else :size="20" class="purple-text" /><div><strong>{{ job.target }}</strong><p>{{ job.message || labelState(job.state) }}</p><small>{{ formatDate(job.createdAt) }}<span v-if="job.rawExitCode !== null"> · 退出码 {{ job.rawExitCode }} / 0x{{ job.rawExitCode.toString(16).toUpperCase() }}</span></small></div><span :class="['status-badge', statusClass(job.state)]">{{ labelState(job.state) }}</span></div><div class="install-job-actions"><button class="text-button" @click="viewInstallLog(job)"><Terminal :size="14" />查看诊断日志</button><button v-if="['verification_required', 'reboot_required'].includes(job.state)" class="text-button" @click="recheckJob(job)"><RefreshCw :size="14" />确认结束并复检</button><button v-else-if="pendingInstallStates.has(job.state)" class="text-button warning-text" @click="cancelInstall(job)">请求取消</button></div></article></div></section>
        </template>

        <template v-else-if="page === 'history'">
          <section class="surface history-panel"><div class="filter-toolbar"><div class="search-field"><Search :size="17" /><input v-model="historyQuery" placeholder="搜索脚本名称或路径…" aria-label="搜索运行历史" /></div><select v-model="historyState" aria-label="按运行结果筛选"><option value="">所有结果</option><option value="succeeded">运行成功</option><option value="failed">失败</option><option value="cancelled">已取消 / 已停止</option><option value="timed_out">已超时</option><option value="interrupted">异常中断</option><option value="spawn_failed">启动失败</option></select><select v-model="historyDays" aria-label="按时间筛选"><option value="">全部时间</option><option value="1">最近 24 小时</option><option value="7">最近 7 天</option><option value="30">最近 30 天</option></select></div><div v-if="historyRuns.length" class="table-scroll"><table><thead><tr><th>脚本 / 执行环境</th><th>开始时间</th><th>耗时</th><th>状态</th><th>退出码</th><th>操作</th></tr></thead><tbody><tr v-for="run in historyRuns" :key="run.id"><td><strong>{{ run.scriptName }}</strong><small :title="run.runtimePath">{{ run.runtimePath }}</small></td><td>{{ formatDate(run.startedAt, true) }}</td><td>{{ duration(run) }}</td><td><span :class="['status-badge', statusClass(run.state)]">{{ labelState(run.state) }}</span></td><td class="mono">{{ run.rootExitCode ?? '—' }}</td><td><button class="text-button" @click="viewHistory(run)">查看详情<ChevronRight :size="13" /></button></td></tr></tbody></table></div><div v-else class="empty-state"><span class="empty-icon"><History :size="29" /></span><h3>{{ runs.length ? '没有符合条件的记录' : '暂无运行记录' }}</h3><p>执行结束后，可在这里回放日志、查看耗时与退出结果。</p></div></section>
        </template>

        <template v-else-if="page === 'settings'">
          <div class="surface settings-card font-setting"><label for="ui-font-size">界面字号</label><select id="ui-font-size" v-model.number="uiFontSize"><option :value="14">14 px</option><option :value="16">16 px（默认）</option><option :value="18">18 px</option><option :value="20">20 px</option></select></div><div class="settings-layout"><div class="settings-column"><section class="surface settings-card"><div class="settings-card-heading"><span class="overview-icon lilac"><SlidersHorizontal :size="20" /></span><div><h2>运行偏好</h2><p>新添加脚本的默认值，不改变已有绑定。</p></div></div><div class="setting-row"><label for="max-concurrent">同时运行任务上限<small>同一个脚本默认不允许重复运行</small></label><select id="max-concurrent" v-model.number="settings.maxConcurrent"><option v-for="n in 4" :key="n" :value="n">{{ n }} 个任务</option></select></div><div class="setting-row"><label for="default-mode">默认运行模式<small>内置终端支持输入与按键</small></label><select id="default-mode" v-model="settings.defaultMode"><option value="terminal">内置终端</option><option value="console">系统控制台</option></select></div></section><section class="surface settings-card"><div class="settings-card-heading"><span class="overview-icon sky"><HardDrive :size="20" /></span><div><h2>日志与存储</h2></div></div><div class="setting-row"><label for="retention">历史日志保留时间</label><div class="number-field"><input id="retention" v-model.number="settings.retentionDays" type="number" min="1" max="3650" /><span>天</span></div></div><div class="setting-row"><label for="log-limit">单次日志上限<small>达到上限后继续运行，停止保存新增输出</small></label><div class="number-field"><input id="log-limit" v-model.number="settings.logLimitMib" type="number" min="1" max="1024" /><span>MiB</span></div></div><div class="setting-row"><label for="total-log">日志总配额<small>优先清理最旧的已结束任务，不清理活动任务</small></label><div class="number-field"><input id="total-log" v-model.number="settings.totalLogMib" type="number" min="20" max="102400" /><span>MiB</span></div></div><div class="settings-save"><button class="button primary" :disabled="busy" @click="saveSettings"><Check :size="16" />保存偏好设置</button></div></section></div><div class="settings-column narrow"><section class="surface settings-card"><div class="settings-card-heading"><span class="overview-icon mint"><ShieldCheck :size="20" /></span><div><h2>数据备份</h2></div></div><p class="settings-note">数据库备份仅包含配置与记录。迁移工作台请复制整个程序目录及 data；脚本副本会随 data/scripts 保留。新设备需检测解释器并重建 venv。</p><div class="backup-buttons"><button class="button" @click="backupDatabase"><Download :size="16" />备份数据库</button><button class="button" @click="restoreDatabase"><Upload :size="16" />恢复备份</button></div></section><section class="surface settings-card"><h2>本地数据目录</h2><p class="data-path">{{ appInfo.dataDir || '正在读取…' }}</p><button class="text-button" @click="perform(() => command('open_data_directory'))"><FolderOpen :size="15" />打开数据目录<ArrowUpRight :size="13" /></button></section><section class="about-card"><span class="brand-icon"><Terminal :size="25" /></span><h3>RScriptoBox</h3><div><span>v{{ appInfo.version }}</span><span>Rust + Tauri 2</span><span>Windows x64</span></div></section></div></div>
        </template>
      </div>

    </main>

    <div v-if="dragOver" class="drag-overlay"><div><Upload :size="45" /><h2>放下脚本，加入工作台</h2><p>.py · .ps1 · .cmd · .bat</p></div></div>
    <div class="toast-stack" role="status" aria-live="polite"><div v-for="notice in notices" :key="notice.id" :class="['toast', { error: notice.error }]"><TriangleAlert v-if="notice.error" :size="18" /><CheckCheck v-else :size="18" /><span>{{ notice.message }}</span><button aria-label="关闭提示" @click="notices = notices.filter(n => n.id !== notice.id)"><X :size="15" /></button></div></div>

    <div v-if="editing" class="drawer-overlay" @click.self="!busy && (editing = null)"><section class="config-drawer" role="dialog" aria-modal="true" aria-labelledby="config-title"><header class="drawer-heading"><div><h2 id="config-title">脚本与执行配置</h2></div><button class="icon-button" aria-label="关闭配置" :disabled="busy" @click="editing = null"><X :size="21" /></button></header><div class="config-script"><span :class="['script-type-icon', editing.kind]"><component :is="typeIcon(editing.kind)" :size="24" /></span><div><strong>{{ editing.name }}</strong><small>{{ editing.path }}</small></div></div><div class="drawer-tabs"><button :class="{ selected: editTab === 'general' }" @click="editTab = 'general'">基本信息</button><button :class="{ selected: editTab === 'execution' }" @click="editTab = 'execution'">执行配置</button><button :class="{ selected: editTab === 'inputs' }" @click="editTab = 'inputs'">参数与变量<span>{{ editing.args.length + envRows.length }}</span></button></div><div class="drawer-body"><div v-if="configErrors" class="inline-error"><TriangleAlert :size="16" />{{ configErrors }}</div><template v-if="editTab === 'general'"><label class="form-field">脚本名称<input v-model="editing.name" maxlength="120" /></label><label class="form-field">描述<textarea v-model="editing.description" rows="4" maxlength="4000" placeholder="简单介绍这个脚本的用途…"></textarea></label><label class="form-field">标签<input v-model="editTags" placeholder="例如：开发，工具，备份" /><small>使用逗号分隔，帮助你快速分类查找。</small></label><label class="checkbox-row"><input v-model="editing.favorite" type="checkbox" /><Heart :size="16" />加入我的收藏</label><div class="form-divider"></div><div class="form-field"><span>工作台副本路径</span><div class="readonly-path">{{ editing.path }}</div><small>仅复制入口脚本。关联文件请放入副本所在目录；运行和编辑使用此副本，移除记录保留文件。</small></div><div class="button-row"><button class="button small" @click="scriptAction('open_script_location', editing)"><FolderOpen :size="14" />打开目录</button><button class="button small" @click="scriptAction('edit_script', editing)"><FileCode2 :size="14" />系统编辑器</button></div></template><template v-else-if="editTab === 'execution'"><label class="form-field">绑定解释器<select v-model="editing.runtimeId" @change="changeRuntimeBinding"><option :value="null">请选择明确的解释器</option><option v-for="runtime in compatibleRuntimes" :key="runtime.id" :value="runtime.id">{{ kindNames[runtime.kind] }} {{ runtime.version }} · {{ runtime.architecture }} · {{ labelState(runtime.status) }}</option></select><small>已有绑定不会随 PATH 或系统默认版本变化而切换。</small></label><label v-if="editing.kind === 'powershell'" class="form-field">PowerShell 执行策略<select v-model="editing.executionPolicy"><option :value="null">遵循系统策略（默认）</option><option value="RemoteSigned">仅本次进程 · RemoteSigned</option><option value="Bypass">仅本次进程 · Bypass</option></select><small>仅在你明确选择后应用于本次进程，不修改系统策略，且无法覆盖组织的组策略。请只运行你信任的脚本。Windows PowerShell 5.1 中文源码请使用兼容编码，例如 UTF-8 BOM；输出编码不会改变源码解析。</small></label><div v-if="editing.runtimeId" class="readonly-path compact">{{ runtimes.find(r => r.id === editing!.runtimeId)?.executablePath }}</div><label v-if="editing.kind === 'python'" class="form-field">Python 专用环境<select v-model="editing.environmentId" @change="changeEnvironmentBinding"><option :value="null">使用以上基础解释器</option><option v-for="environment in environments" :key="environment.id" :value="environment.id">{{ environment.interpreterPath }} · {{ labelState(environment.dependencyStatus) }}</option></select><small>选定专用环境后，实际使用它的 python.exe 执行。</small></label><div class="form-field"><label for="script-cwd">工作目录</label><div class="input-action"><input id="script-cwd" v-model="editing.cwd" placeholder="留空使用脚本所在目录" /><button aria-label="选择工作目录" @click="perform(async () => { const path = await pickDirectory(); if (path && editing) editing.cwd = path; })"><FolderOpen :size="17" /></button></div></div><div class="form-field"><span>运行模式</span><div class="mode-options"><button :class="{ selected: editing.runMode === 'terminal' }" @click="editing.runMode = 'terminal'; editing.encoding = 'utf-8'"><Terminal :size="20" /><strong>内置终端</strong><small>直接键入，支持交互并保存输出</small></button><button :class="{ selected: editing.runMode === 'console' }" @click="editing.runMode = 'console'"><ExternalLink :size="20" /><strong>系统控制台</strong><small>支持 input / Read-Host / pause</small></button></div></div><div class="form-columns"><label class="form-field">超时（秒）<input v-model.number="editing.timeoutSeconds" type="number" min="1" placeholder="不限" @change="editing.timeoutSeconds = editing.timeoutSeconds || null" /><small>超时会强制结束受控进程树。</small></label><label class="form-field">{{ editing.runMode === 'terminal' ? '终端传输固定 UTF-8' : '输出编码' }}<select v-model="editing.encoding" :disabled="editing.runMode === 'terminal'"><option value="utf-8">UTF-8</option><option value="system">系统代码页</option><option value="gb18030">GB18030</option></select></label></div><label class="form-field">成功退出码<input v-model="editCodes" placeholder="0, 3010" /><small>用逗号分隔。stderr 有内容并不直接代表运行失败。</small></label></template><template v-else><div class="field-heading"><div><h3>命令行参数</h3><p>每项是一个参数，留空也是有效参数。</p></div><button class="button small" @click="addArgument"><Plus :size="14" />添加参数</button></div><div v-if="!editing.args.length" class="form-empty">没有额外参数，脚本将使用自身默认行为。</div><div v-for="(arg, index) in editing.args" :key="index" class="argument-item"><div class="argument-row"><span class="index-label">{{ index + 1 }}</span><input v-if="typeof arg === 'string'" :value="arg" :aria-label="`第 ${index + 1} 项参数`" placeholder="参数值（允许为空）" @input="editing!.args[index] = ($event.target as HTMLInputElement).value" /><input v-else v-model="arg.secretSlot" :aria-label="`第 ${index + 1} 项敏感槽位`" placeholder="敏感槽位名称" /><button :class="['secret-toggle', { selected: typeof arg !== 'string' }]" :title="typeof arg === 'string' ? '标记敏感输入，仅运行时填写' : '改为普通参数'" @click="toggleArgumentSecret(index)"><ShieldCheck :size="15" /></button><button class="icon-button" aria-label="删除参数" @click="editing.args.splice(index, 1)"><X :size="16" /></button></div><label v-if="typeof arg !== 'string'" class="secret-note"><input v-model="arg.required" type="checkbox" />本次运行时必填 · 值不会保存到配置</label></div><div class="form-divider"></div><div class="field-heading"><div><h3>环境变量</h3><p>继承当前环境，只覆盖本次子进程。</p></div><button class="button small" @click="envRows.push({ key: '', value: '' })"><Plus :size="14" />添加变量</button></div><div v-if="!envRows.length" class="form-empty">未添加覆盖项，将继承当前用户环境。</div><div v-for="(row, index) in envRows" :key="index" class="argument-item"><div class="env-row"><input v-model="row.key" placeholder="变量名" aria-label="环境变量名称" /><span>=</span><input v-if="typeof row.value === 'string'" v-model="row.value" placeholder="变量值" aria-label="环境变量值" /><input v-else v-model="row.value.secretSlot" placeholder="敏感槽位名称" aria-label="敏感环境变量槽位" /><button :class="['secret-toggle', { selected: typeof row.value !== 'string' }]" title="切换敏感输入" @click="toggleEnvSecret(index)"><ShieldCheck :size="15" /></button><button class="icon-button" aria-label="删除环境变量" @click="envRows.splice(index, 1)"><X :size="15" /></button></div><label v-if="typeof row.value !== 'string'" class="secret-note"><input v-model="row.value.required" type="checkbox" />运行时必填 · 值仅保存在本次内存</label></div><div class="form-hint"><ShieldCheck :size="17" /><span>点击盾牌标记密码或令牌。普通变量会保存到本地数据库；敏感值仅运行时填写。脚本主动输出的敏感内容仍可能进入日志。</span></div></template></div><div class="execution-preview"><span>执行概要 · 实际参数由后端安全构建</span><code>{{ executionPreview }}</code></div><footer class="drawer-footer"><button class="button" :disabled="busy" @click="editing = null">取消</button><div class="spacer"></div><button class="button" :disabled="busy" @click="runTemporary"><Play :size="14" />仅运行一次</button><button class="button primary" :disabled="busy" @click="saveConfig"><Check :size="16" />保存配置</button></footer></section></div>

    <div v-if="registerDialog" class="modal-overlay" @click.self="!busy && (registerDialog = false)"><form class="modal" @submit.prevent="registerRuntime"><div class="modal-heading"><span class="overview-icon lilac"><Box :size="23" /></span><button class="icon-button" type="button" aria-label="关闭" @click="registerDialog = false"><X :size="20" /></button></div><h2>登记已有解释器</h2><p class="modal-description">选择实际解释器文件，应用会执行固定探针验证版本与可用性。</p><label class="form-field">环境类型<select v-model="registerKind"><option value="python">Python / 已有 venv / Conda</option><option value="powershell">PowerShell 7</option><option value="windows_powershell">Windows PowerShell 5.1</option><option value="cmd">CMD</option></select></label><div class="form-field"><label for="runtime-path">解释器路径</label><div class="input-action"><input id="runtime-path" v-model="registerPath" required placeholder="选择实际的 .exe 文件" /><button type="button" aria-label="选择解释器" @click="chooseRuntimePath"><FolderOpen :size="17" /></button></div><small>请选择你信任的本地程序。验证会启动该解释器。</small></div><div class="modal-footer"><button class="button" type="button" @click="registerDialog = false">取消</button><button class="button primary" type="submit" :disabled="busy || !registerPath"><ShieldCheck :size="15" />验证并登记</button></div></form></div>
    <div v-if="installPlan" class="modal-overlay" @click.self="!busy && (installPlan = null)"><section class="modal wide-modal" role="dialog" aria-modal="true" aria-labelledby="install-title"><div class="modal-heading"><span class="overview-icon lilac"><Download :size="23" /></span><button class="icon-button" aria-label="关闭" @click="installPlan = null"><X :size="20" /></button></div><h2 id="install-title">安装计划 · {{ installPlan.product }}</h2><p class="modal-description">请核对来源与安装范围。完成安装后仍需手动启动脚本。</p><dl class="plan-details"><dt>版本 / 架构</dt><dd>{{ installPlan.version }} · {{ installPlan.architecture }}</dd><dt>安装范围</dt><dd>{{ installPlan.scope }}</dd><dt>来源 / 包标识</dt><dd>{{ installPlan.source }} / {{ installPlan.packageId }}</dd><dt>许可说明</dt><dd class="mono">{{ installPlan.licenseUrl }}</dd></dl><ol class="install-steps"><li v-for="action in installPlan.actions" :key="action">{{ action }}</li></ol><div v-if="installPlan.warning" class="inline-warning"><TriangleAlert :size="17" /><span>{{ installPlan.warning }}</span></div><label class="checkbox-row agreement"><input v-model="installAccepted" type="checkbox" />我已查看安装计划，同意相关来源与软件许可，并知悉可能出现 UAC 或系统交互。</label><div class="modal-footer"><button class="button" @click="installPlan = null">返回</button><button class="button primary" :disabled="!installAccepted || busy" @click="startInstall"><Download :size="15" />确认并安装</button></div></section></div>
    <div v-if="venvDialog" class="modal-overlay" @click.self="!busy && (venvDialog = false)"><form class="modal" @submit.prevent="createVenv"><div class="modal-heading"><span class="overview-icon lilac"><Box :size="23" /></span><button class="icon-button" type="button" aria-label="关闭" @click="venvDialog = false"><X :size="20" /></button></div><h2>创建 Python 专用环境</h2><p class="modal-description">在应用数据目录创建新的 venv，验证成功后才绑定脚本。原环境不会被删除或覆盖。</p><label class="form-field">目标脚本<select v-model="venvScript" required><option value="">选择 Python 脚本</option><option v-for="script in pythonScripts" :key="script.id" :value="script.id">{{ script.name }}</option></select></label><label class="form-field">基础解释器<select v-model="venvRuntime" required><option value="">选择已验证的 Python</option><option v-for="runtime in pythonRuntimes" :key="runtime.id" :value="runtime.id">Python {{ runtime.version }} · {{ runtime.architecture }} · {{ runtime.executablePath }}</option></select></label><div class="modal-footer"><button class="button" type="button" @click="venvDialog = false">取消</button><button class="button primary" type="submit" :disabled="busy || !venvScript || !venvRuntime"><Plus :size="15" />创建并验证</button></div></form></div>
    <div v-if="selectedJob" class="modal-overlay" @click.self="selectedJob = null"><section class="modal log-modal" role="dialog" aria-modal="true" aria-labelledby="install-log-title"><div class="modal-heading"><span class="overview-icon lilac"><Terminal :size="23" /></span><button class="icon-button" aria-label="关闭" @click="selectedJob = null"><X :size="20" /></button></div><h2 id="install-log-title">{{ selectedJob.target }}</h2><div class="install-log-meta"><span :class="['status-badge', statusClass(selectedJob.state)]">{{ labelState(selectedJob.state) }}</span><button class="text-button" @click="viewInstallLog(selectedJob)"><RefreshCw :size="14" />刷新日志</button></div><pre class="install-log-text">{{ installLog || '暂无诊断输出。' }}</pre><p class="modal-description">{{ selectedJob.message }}</p></section></div>
    <div v-if="pendingRun" class="modal-overlay" @click.self="!busy && (pendingRun = null, secretValues = {})"><form class="modal" @submit.prevent="submitSecrets"><div class="modal-heading"><span class="overview-icon lilac"><ShieldCheck :size="23" /></span><button type="button" class="icon-button" aria-label="关闭" @click="pendingRun = null; secretValues = {}"><X :size="20" /></button></div><h2>填写本次敏感输入</h2><p class="modal-description">{{ pendingRun.script.name }} · 输入值仅用于本次运行，不保存到配置或历史元数据。</p><label v-for="slot in currentSecretSlots" :key="slot.name" class="form-field">{{ slot.name }}<span v-if="slot.required" class="required-label">必填</span><input v-model="secretValues[slot.name]" type="password" :required="slot.required" autocomplete="off" placeholder="仅本次使用" /></label><div class="form-hint"><Info :size="16" /><span>命令行中的秘密可能被本机有权限的进程查看；脚本输出的秘密仍可能进入日志。</span></div><div class="modal-footer"><button type="button" class="button" @click="pendingRun = null; secretValues = {}">取消</button><button type="submit" class="button primary" :disabled="busy"><Play :size="14" />确认运行</button></div></form></div>
    <div v-if="confirmation" class="modal-overlay confirm-overlay" @click.self="!busy && (confirmation = null)"><section class="modal confirm-modal" role="alertdialog" aria-modal="true" aria-labelledby="confirm-title"><div class="modal-heading"><span :class="['overview-icon', confirmation.danger ? 'rose' : 'lilac']"><TriangleAlert v-if="confirmation.danger" :size="24" /><Info v-else :size="24" /></span><button class="icon-button" aria-label="关闭" :disabled="busy" @click="confirmation = null"><X :size="20" /></button></div><h2 id="confirm-title">{{ confirmation.title }}</h2><p class="confirm-message">{{ confirmation.message }}</p><div class="modal-footer"><button class="button" :disabled="busy" @click="confirmation = null">取消</button><button :class="['button', confirmation.danger ? 'danger' : 'primary']" :disabled="busy" @click="acceptConfirmation"><LoaderCircle v-if="busy" class="spin" :size="15" />{{ confirmation.label }}</button></div></section></div>
  </div>
</template>
