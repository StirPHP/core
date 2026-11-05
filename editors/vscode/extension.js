// @ts-check

import * as vscode from 'vscode';
// vscode-languageclient is CommonJS: take its exports from the default import
import languageClient from 'vscode-languageclient/node';
import { join } from 'node:path';
import { cleanup, install, installed, latestRelease, targetTriple } from './server.js';

const { LanguageClient } = languageClient;

/** How often to look for a new `stirphp` (also checked on demand). */
const UPDATE_INTERVAL_MS = 6 * 60 * 60 * 1000;

/** @type {import('vscode-languageclient/node').LanguageClient | undefined} */
let client;
/** @type {vscode.ExtensionContext} */
let ctx;
/** @type {vscode.LogOutputChannel} */
let log;
/** The update check in progress, if any. */
/** @type {Promise<void> | undefined} */
let pendingCheck;

function config() {
  return vscode.workspace.getConfiguration('stirphp');
}

/** @returns {import('./server.js').Channel} */
function channel() {
  return config().get('server.channel') === 'nightly' ? 'nightly' : 'stable';
}

function binDir() {
  return join(ctx.globalStorageUri.fsPath, 'bin');
}

/**
 * The `stirphp` to run: `stirphp.server.path` when set; otherwise the build this
 * extension manages, downloaded first if there is none yet; otherwise `stirphp`
 * from the PATH.
 * @returns {Promise<string>}
 */
async function resolveCommand() {
  const explicit = config().get('server.path', '').trim();
  if (explicit) {
    log.appendLine(`Using stirphp.server.path: ${explicit}`);
    return explicit;
  }
  const target = targetTriple();
  if (!target) {
    log.appendLine(`No stirphp build for ${process.platform}-${process.arch}; using stirphp from the PATH`);
    return 'stirphp';
  }
  const current = await installed(binDir());
  if (current && current.channel === channel()) {
    log.appendLine(`Using stirphp ${current.label} (${current.path})`);
    scheduleUpdateCheck();
    return current.path;
  }
  try {
    const fresh = await vscode.window.withProgress(
      { location: vscode.ProgressLocation.Notification, title: 'StirPHP: downloading the language server…' },
      async () => install(await latestRelease(channel(), target), channel(), binDir()),
    );
    log.appendLine(`Installed stirphp ${fresh.label} (${fresh.path})`);
    await ctx.globalState.update('stirphp.lastUpdateCheck', Date.now());
    await cleanup(binDir(), fresh);
    return fresh.path;
  } catch (error) {
    log.appendLine(`Download failed: ${error}`);
    if (current) {
      // e.g. offline after switching channels: keep using what we have
      log.appendLine(`Using the installed stirphp ${current.label} instead`);
      return current.path;
    }
    vscode.window.showWarningMessage(
      `StirPHP: could not download the language server (${error}). Trying \`stirphp\` from the PATH.`,
    );
    return 'stirphp';
  }
}

/** Looks for a newer build in the background, unless one was checked recently. */
function scheduleUpdateCheck() {
  if (!config().get('server.autoUpdate', true)) return;
  const last = ctx.globalState.get('stirphp.lastUpdateCheck', 0);
  if (Date.now() - last < UPDATE_INTERVAL_MS) return;
  checkForUpdate(false);
}

/**
 * Installs a newer build of the current channel, then offers to restart the
 * server with it. A check while another runs waits for it; an interactive one
 * then checks again itself.
 * @param {boolean} interactive  report "up to date" and errors (the command)
 */
async function checkForUpdate(interactive) {
  while (pendingCheck) {
    await pendingCheck;
    if (!interactive) return;
  }
  pendingCheck = runUpdateCheck(interactive).finally(() => {
    pendingCheck = undefined;
  });
  return pendingCheck;
}

/** @param {boolean} interactive */
async function runUpdateCheck(interactive) {
  const target = targetTriple();
  if (!target) return;
  try {
    const current = await installed(binDir());
    const latest = await latestRelease(channel(), target);
    await ctx.globalState.update('stirphp.lastUpdateCheck', Date.now());
    if (current && current.channel === channel() && current.id === latest.id) {
      log.appendLine(`stirphp ${current.label} is up to date`);
      if (interactive) vscode.window.showInformationMessage(`StirPHP: stirphp ${current.label} is up to date.`);
      return;
    }
    log.appendLine(`Downloading stirphp ${latest.label}`);
    const fresh = await install(latest, channel(), binDir());
    log.appendLine(`Installed stirphp ${fresh.label} (${fresh.path})`);
    // not awaited: an unanswered prompt must not hold up later checks
    vscode.window
      .showInformationMessage(`StirPHP: stirphp ${fresh.label} is installed.`, 'Restart the language server')
      .then((choice) => (choice ? restart() : undefined));
  } catch (error) {
    log.appendLine(`Update check failed: ${error}`);
    if (interactive) vscode.window.showErrorMessage(`StirPHP: update check failed: ${error}`);
  }
}

/** @param {string} command */
function createClient(command) {
  /** @type {import('vscode-languageclient/node').ServerOptions} */
  const serverOptions = {
    run: { command, args: ['lsp'] },
    debug: { command, args: ['lsp'], options: { env: { ...process.env, STIRPHP_LOG: 'debug' } } },
  };
  const documentSelector = [{ scheme: 'file', language: 'php' }];
  /** @type {import('vscode-languageclient/node').LanguageClientOptions} */
  const clientOptions = {
    documentSelector,
    outputChannel: log,
    // the server registers its own file watchers
    // in an untrusted workspace, scripts and PHPStan never run, and the server doesn't ask about them
    initializationOptions: {
      workspaceTrusted: vscode.workspace.isTrusted,
      phpstan: {
        enable: config().get('phpstan.enable', 'auto'),
        path: config().get('phpstan.path', ''),
        configuration: config().get('phpstan.configuration', ''),
        level: config().get('phpstan.level', ''),
        memoryLimit: config().get('phpstan.memoryLimit', ''),
        timeout: config().get('phpstan.timeout', 0),
      },
    },
  };
  return new LanguageClient('stirphp', 'StirPHP', serverOptions, clientOptions);
}

async function start() {
  const command = await resolveCommand();
  client = createClient(command);
  try {
    await client.start();
    // the running build is the current one: older ones can go
    const current = await installed(binDir());
    if (current && current.path === command) await cleanup(binDir(), current);
  } catch (error) {
    const choice = await vscode.window.showErrorMessage(
      `StirPHP: could not start the language server (\`${command} lsp\`): ${error}`,
      'Open Settings',
    );
    if (choice === 'Open Settings') {
      await vscode.commands.executeCommand('workbench.action.openSettings', 'stirphp.server');
    }
  }
}

async function stop() {
  if (client) {
    const c = client;
    client = undefined;
    await c.stop();
  }
}

async function restart() {
  await stop();
  await start();
}

/** @param {vscode.ExtensionContext} context */
export async function activate(context) {
  ctx = context;
  log = vscode.window.createOutputChannel('StirPHP', { log: true });
  context.subscriptions.push(
    log,
    vscode.commands.registerCommand('stirphp.restartServer', restart),
    vscode.commands.registerCommand('stirphp.checkForUpdates', async () => {
      if (config().get('server.path', '').trim()) {
        vscode.window.showInformationMessage('StirPHP: `stirphp.server.path` is set, so the extension does not manage stirphp.');
        return;
      }
      await checkForUpdate(true);
    }),
    vscode.workspace.onDidGrantWorkspaceTrust(restart),
    vscode.workspace.onDidChangeConfiguration(async (e) => {
      if (e.affectsConfiguration('stirphp.server') || e.affectsConfiguration('stirphp.phpstan')) {
        await restart();
      }
    }),
  );
  await start();
}

export function deactivate() {
  return stop();
}
