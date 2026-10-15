<script setup lang="ts">
import { onMounted, onUnmounted, ref, watch } from 'vue';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import { command } from './api';

const props = defineProps<{ runId: string; text: string; interactive: boolean; fontSize: number; autoScroll: boolean }>();
const emit = defineEmits<{ error: [message: string] }>();
const host = ref<HTMLElement>();
let terminal: Terminal | undefined;
const fit = new FitAddon();
let observer: ResizeObserver | undefined;
let previous = '';
let pending: string[] = [];
let sending = false;
let disposed = false;
let frame = 0;
let writing = false;
function fitTerminal() {
  if (!host.value?.clientWidth || !terminal) return;
  fit.fit();
  if (props.interactive) void command('resize_terminal', { id: props.runId, cols: Math.min(500, Math.max(4, terminal.cols)), rows: Math.min(300, Math.max(2, terminal.rows)) }).catch(() => {});
}
function render() {
  if (!terminal || writing) return;
  const append = props.text.startsWith(previous);
  if (!append) terminal.reset();
  const text = append ? props.text.slice(previous.length) : props.text;
  previous = props.text;
  if (text) {
    writing = true;
    terminal.write(text, () => { writing = false; if (props.autoScroll) terminal?.scrollToBottom(); if (!disposed) render(); });
  }
}
async function send(data: string) {
  if (!props.interactive || disposed) return;
  // Bound paste input before enqueueing; split at Unicode character boundaries.
  if (data.length > 32768 || pending.length > 32) { emit('error', '粘贴内容过长或输入尚未发送完，请分批输入。'); return; }
  let chunk = '';
  for (const character of data) {
    chunk += character;
    if (chunk.length >= 512) { pending.push(chunk); chunk = ''; }
  }
  if (chunk) pending.push(chunk);
  if (sending) return;
  sending = true;
  try {
    while (pending.length && props.interactive && !disposed) await command('write_terminal_input', { id: props.runId, text: pending.shift()! });
  } catch (error) { pending = []; if (!disposed) emit('error', String(error)); }
  finally { sending = false; }
}
onMounted(() => {
  terminal = new Terminal({ fontSize: props.fontSize, fontFamily: '"Cascadia Mono", Consolas, "Microsoft YaHei UI", monospace', lineHeight: 1.2, cursorBlink: props.interactive, disableStdin: !props.interactive, scrollback: 10000, screenReaderMode: true, theme: { background: '#101218', foreground: '#e4e7ef', cursor: '#b69cff', selectionBackground: '#57447d', black: '#222632', red: '#f38b98', green: '#90d6aa', yellow: '#efd28c', blue: '#93b9f7', magenta: '#c0a2f3', cyan: '#86d3dd', white: '#e4e7ef' } });
  // Script output must not open browser windows through OSC 8 links.
  terminal.options.linkHandler = { activate: () => {} };
  terminal.loadAddon(fit);
  terminal.open(host.value!);
  terminal.onData(data => { void send(data); });
  terminal.attachCustomKeyEventHandler(event => !(event.ctrlKey && event.shiftKey && ['c', 'v'].includes(event.key.toLowerCase())));
  observer = new ResizeObserver(() => { cancelAnimationFrame(frame); frame = requestAnimationFrame(fitTerminal); });
  observer.observe(host.value!);
  fitTerminal(); render();
});
watch(() => props.text, render);
watch(() => props.fontSize, value => { if (terminal) terminal.options.fontSize = value; fitTerminal(); });
watch(() => props.interactive, value => { if (terminal) { terminal.options.disableStdin = !value; terminal.options.cursorBlink = value; } if (!value) pending = []; fitTerminal(); });
onUnmounted(() => { disposed = true; pending = []; observer?.disconnect(); cancelAnimationFrame(frame); terminal?.dispose(); terminal = undefined; });
function plainText() {
  const buffer = terminal?.buffer.active;
  if (!buffer) return '';
  return Array.from({ length: buffer.length }, (_, index) => buffer.getLine(index)?.translateToString(true) || '').join('\n').trimEnd();
}
defineExpose({ plainText });
</script>

<template><div ref="host" class="interactive-terminal" :aria-label="interactive ? '交互终端，点击后可直接键入' : '终端输出回放'"></div></template>
