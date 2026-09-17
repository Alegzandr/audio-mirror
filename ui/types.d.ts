// Types of the data exchanged with the Rust side, for `tsc --checkJs`.
// Keep in sync with the `Serialize` structs and `#[tauri::command]` handlers.

/** `config::OutputConfig` */
interface OutputConfig {
  id: string;
  name: string;
  enabled: boolean;
  /** Fader position, 0..1. */
  fader: number;
  muted: boolean;
}

/** `config::AppConfig` */
interface AppConfig {
  source: string;
  outputs: OutputConfig[];
}

/** `audio::SourceInfo` */
interface SourceInfo {
  id: string;
  name: string;
  kind: "desktop" | "loopback" | "capture";
  is_default: boolean;
  captures_output: string | null;
}

/** `audio::OutputInfo` */
interface OutputInfo {
  id: string;
  name: string;
  is_default: boolean;
}

/** `audio::DeviceList` */
interface DeviceList {
  sources: SourceInfo[];
  outputs: OutputInfo[];
}

/** `lib::Snapshot` */
interface Snapshot {
  version: string;
  config: AppConfig;
  devices: DeviceList;
  autostart: boolean;
  update_ready: string | null;
}

/** `audio::NodeState` */
type NodeState = "idle" | "starting" | "playing" | "error" | "blocked";

/** `audio::message::Message` */
interface EngineMessage {
  /** Translation key, looked up in the table of `ui/i18n.js`. */
  code: string;
  /** English wording, shown when the code has no translation. */
  text: string;
  /** Untranslatable part, such as an `HRESULT` or a system error. */
  detail: string | null;
}

/** `audio::SourceStatus` */
interface SourceStatus {
  state: NodeState;
  message: EngineMessage | null;
  format: string | null;
  peak: number;
}

/** `audio::OutputStatus` */
interface OutputStatus extends SourceStatus {
  id: string;
}

/** `audio::Status` */
interface Status {
  running: boolean;
  source: SourceStatus;
  outputs: OutputStatus[];
  /** Bumped when the system reports a device change. */
  devices_revision: number;
}

/** `updater::Progress` */
interface Progress {
  stage: "checking" | "downloading" | "installing" | "restarting";
  percent: number | null;
}

/** Commands: arguments and result. */
interface Commands {
  snapshot: { args: void; result: Snapshot };
  status: { args: void; result: Status };
  set_source: { args: { id: string }; result: void };
  set_output_enabled: { args: { id: string; name: string; enabled: boolean }; result: void };
  set_output_volume: { args: { id: string; name: string; fader: number; persist: boolean }; result: void };
  set_output_muted: { args: { id: string; name: string; muted: boolean }; result: void };
  set_autostart: { args: { enabled: boolean }; result: boolean };
  hide_panel: { args: void; result: void };
  restart: { args: void; result: void };
  quit: { args: void; result: void };
  update_progress: { args: void; result: Progress };
}

type Command = keyof Commands;
type CommandArgs<K extends Command> = Commands[K]["args"] extends void ? [] : [Commands[K]["args"]];
type CommandResult<K extends Command> = Commands[K]["result"];
type Invoke = <K extends Command>(cmd: K, ...args: CommandArgs<K>) => Promise<CommandResult<K>>;

/** Events emitted by the Rust side. */
interface Events {
  "update-ready": string;
}

type Listen = <E extends keyof Events>(
  event: E,
  handler: (event: { payload: Events[E] }) => void,
) => Promise<unknown>;

interface Window {
  /** Present inside Tauri (`app.withGlobalTauri`), absent in a browser preview. */
  __TAURI__?: {
    core: { invoke: Invoke };
    event: { listen: Listen };
  };
  /** Browser preview state, see `demoInvoke` in app.js. */
  __demo?: { config: AppConfig };
  /** Interface language from the system locale, injected by `i18n.rs`. */
  __AUDIO_MIRROR_LANG__?: string;
  /** Set by `ui/i18n.js`, which loads before the page script. */
  I18n: I18n;
  /** Set by `ui/view.js`, which loads before the page script. */
  View: View;
}

/** `ui/view.js`: what the panel shows, worked out without a DOM. */
interface View {
  faderToDb(def: number): number;
  formatDb(db: number): string;
  peakToDb(peak: number): number;
  meterTarget(peak: number): number;
  meterFall(level: number, target: number): number;
  meterDb(level: number): number;
  sourceName(source: SourceInfo): string;
  outputRows(devices: DeviceList, config: AppConfig): OutputInfo[];
  capturedOutput(devices: DeviceList, config: AppConfig): string | null;
  runState(status: Status | null, rows: OutputInfo[]): { text: string; tone: string };
  outputState(
    status: OutputStatus | undefined,
    row: { enabled: boolean; muted: boolean },
  ): { label: string; tone: string; detail: string };
  retrying(message: EngineMessage | null): string;
}

/** `ui/i18n.js` */
interface I18n {
  lang: string;
  t(key: string, vars?: Record<string, string | number>): string;
  engineMessage(message: EngineMessage | null): string;
  formatNumber(n: number): string;
  keys(): string[];
}
