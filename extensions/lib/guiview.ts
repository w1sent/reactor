/**
 * guiview -- the vocabulary reactor's extensions share for the GUI contract
 * (ADR-0032, gui/SPEC.md §4): how a view is detected, how it is emitted, and
 * how the GUI's actions come back.
 *
 * The contract is reactor-specific, additive, and transparent: it rides only
 * channels that already exist in pi's RPC mode. Extensions detect the GUI
 * through the environment (`REACTOR_GUI=1`, set by reactor-gui when it spawns
 * pi -- the "sends something first" of ADR-0032); they emit views as
 * `setWidget` lines whose first line carries a marker + JSON and whose
 * remaining lines are a human-readable fallback; and the GUI answers actions
 * by invoking the extension's own event command through RPC `prompt`
 * (verified transcript-free: pi returns "no prompt to send").
 *
 * Stateless by contract (ADR-0029): pure functions over (args), no module
 * state, no caches. The facts stay in the extension and the CLI; this module
 * carries presentation.
 */

/** The env var reactor-gui sets when it spawns pi — the handshake. */
export const GUI_ENV = "REACTOR_GUI";

/** The marker prefixing line 0 of an envelope, plus the schema version. */
export const MARKER = "REACTOR-GUI-VIEW";
export const VERSION = 1;

/** The `widgetKey` prefix that identifies a view envelope. */
export const KEY_PREFIX = "reactor:";

/**
 * Whether the extension is running under reactor-gui: the GUI's handshake is
 * the environment it set on the child. TUI mode and plain-RPC clients do not
 * set it, so nothing changes for them -- the mode matrix of gui/SPEC.md §4.1.
 */
export function isGuiMode(): boolean {
	return process.env[GUI_ENV] !== undefined && process.env[GUI_ENV] !== "";
}

/** Where a view renders: a sheet over the transcript, or a docked panel. */
export type Placement = "overlay" | "side";

/** One table cell: text plus an optional pi-vocabulary colour (§7's mapping). */
export interface Cell {
	text: string;
	color?: string;
}

/** A row-level action the GUI renders as a button (§4.2). */
export interface ViewAction {
	id: string;
	label: string;
	disabled?: boolean;
}

/** One primitive of schema v1: exactly what the selector and guide need. */
export type ViewBody =
	| {
			table: {
				columns: { id: string; title: string; width?: number }[];
				rows: {
					id: string;
					cells: Record<string, Cell>;
					actions?: ViewAction[];
				}[];
			};
	  }
	| {
			list: {
				items: { id: string; label: string; color?: string; actions?: ViewAction[] }[];
			};
	  }
	| { detail: { body: string } };

/** The payload of one view envelope (§4.2, schema v1). */
export interface ViewPayload {
	v: number;
	view: string;
	title: string;
	/** The extension's own event command — the GUI never guesses it. */
	command: string;
	placement: Placement;
	footer?: string;
	/** The body: exactly one of table | list | detail. */
	[body: string]: unknown;
}

/**
 * Build a view payload. `command` is the event command the extension
 * registered (typically `<existing-name>-event`); the GUI sends the action
 * payloads there via `prompt`.
 */
export function buildPayload(options: {
	view: string;
	title: string;
	command: string;
	placement?: Placement;
	footer?: string;
	body: ViewBody;
}): ViewPayload {
	const { view, title, command, placement = "overlay", footer, body } = options;
	const payload: ViewPayload = {
		v: VERSION,
		view,
		title,
		command,
		placement,
		...body,
	};
	if (footer !== undefined) payload.footer = footer;
	return payload;
}

/**
 * The `setWidget` lines for one view: line 0 is the marker + JSON, the
 * remaining lines are the readable fallback that every other client renders
 * (transparency by construction, ADR-0032). The fallback is mandatory: a
 * payload without one hides data from text clients.
 */
export function buildWidgetLines(payload: ViewPayload, fallback: string[]): string[] {
	if (fallback.length === 0) {
		throw new Error(`guiview: view "${payload.view}" emitted without fallback lines`);
	}
	return [`${MARKER} v${VERSION} ${JSON.stringify(payload)}`, ...fallback];
}

/**
 * Parse an envelope out of a `setWidget` key + lines. `undefined` when the
 * widget is not an envelope -- a plain widget renders as text, unchanged.
 * The GUI's counterpart is `reactor-gui/src/contract.rs`; the two must agree
 * on marker, version and shape, and both are pinned by tests.
 */
export function parseView(
	widgetKey: string,
	lines: string[],
): { viewId: string; payload: ViewPayload } | undefined {
	if (!widgetKey.startsWith(KEY_PREFIX)) return undefined;
	const viewId = widgetKey.slice(KEY_PREFIX.length);
	const first = lines[0];
	if (first === undefined) return undefined;
	const jsonPart = first.replace(`${MARKER} v${VERSION} `, "");
	if (jsonPart === first) return undefined;
	let payload: ViewPayload;
	try {
		payload = JSON.parse(jsonPart) as ViewPayload;
	} catch {
		return undefined;
	}
	if (payload?.v !== VERSION || typeof payload.command !== "string") return undefined;
	if (!payload.command.startsWith("/")) return undefined;
	return { viewId, payload };
}

/**
 * The message the GUI sends back for one user action -- the extension
 * command invoked through RPC `prompt`. `row` is the row id when the action
 * came from a table row.
 */
export function eventCommand(command: string, viewId: string, action: string, row?: string): string {
	const payload: Record<string, string> = { view: viewId, action };
	if (row !== undefined) payload.row = row;
	return `${command} ${JSON.stringify(payload)}`;
}

/** The `widgetKey` for a view id. */
export function widgetKey(viewId: string): string {
	return `${KEY_PREFIX}${viewId}`;
}

/** One parsed inbound action: the event command's argument (ADR-0032). */
export interface IncomingEvent {
	view: string;
	action: string;
	row?: string;
}

/**
 * Parse an event command's arguments back into the action. The GUI sends
 * `<command> {"view":…,"action":…,"row":…}`; anything else -- a bare word, a
 * malformed payload -- is `undefined`, and the extension ignores it.
 */
export function parseEventPayload(args: string): IncomingEvent | undefined {
	const space = args.indexOf(" ");
	if (space === -1) return undefined;
	try {
		const payload = JSON.parse(args.slice(space + 1)) as Record<string, unknown>;
		if (typeof payload.view !== "string" || typeof payload.action !== "string") {
			return undefined;
		}
		const event: IncomingEvent = { view: payload.view, action: payload.action };
		if (typeof payload.row === "string") event.row = payload.row;
		return event;
	} catch {
		return undefined;
	}
}


/**
 * Human-readable fallback lines for the two shapes this contract ships: a
 * table renders aligned columns, a list renders one line per item. The
 * fallback is what the TUI, `pi --mode rpc` under another client, and any
 * text widget surface shows -- it must be useful on its own (§4.2).
 */
export function fallbackLines(payload: ViewPayload): string[] {
	if ("table" in payload) {
		const { columns, rows } = payload.table;
		const width = Math.max(1, ...columns.map((c) => (c.width ?? 12)));
		const head = columns.map((c) => (c.title || c.id).padEnd(c.width ?? 12)).join("  ").trimEnd();
		const lines = [head, "-".repeat(Math.min(60, head.length))];
		for (const row of rows) {
			lines.push(
				columns
					.map((c) => (row.cells[c.id]?.text ?? "").padEnd(c.width ?? 12))
					.join("  ")
					.trimEnd(),
			);
		}
		if (payload.footer) lines.push(payload.footer);
		return lines;
	}
	if ("list" in payload) {
		const lines = payload.list.items.map((item) => item.label);
		if (payload.footer) lines.push(payload.footer);
		return lines;
	}
	if ("detail" in payload) {
		const lines = payload.detail.body.split("\n");
		if (payload.footer) lines.push(payload.footer);
		return lines;
	}
	return ["(view with no body)"];
}