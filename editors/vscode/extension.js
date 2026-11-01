// @ts-check

import * as vscode from 'vscode';
// vscode-languageclient is CommonJS: take its exports from the default import
import languageClient from 'vscode-languageclient/node';

const { LanguageClient } = languageClient;

/** @type {import('vscode-languageclient/node').LanguageClient | undefined} */
let client;

/** @returns {import('vscode-languageclient/node').LanguageClient} */
function createClient() {
  const config = vscode.workspace.getConfiguration('sphp');
  const command = config.get('server.path', 'sphp');
  /** @type {import('vscode-languageclient/node').ServerOptions} */
  const serverOptions = {
    run: { command, args: ['lsp'] },
    debug: { command, args: ['lsp'], options: { env: { ...process.env, SPHP_LOG: 'debug' } } },
  };
  const documentSelector = [{ scheme: 'file', language: 'sphp' }];
  if (config.get('php.enable', false)) {
    documentSelector.push({ scheme: 'file', language: 'php' });
  }
  /** @type {import('vscode-languageclient/node').LanguageClientOptions} */
  const clientOptions = {
    documentSelector,
    // the server registers its own file watchers
  };
  return new LanguageClient('sphp', 'StirPHP', serverOptions, clientOptions);
}

async function start() {
  client = createClient();
  try {
    await client.start();
  } catch (error) {
    const path = vscode.workspace.getConfiguration('sphp').get('server.path', 'sphp');
    const choice = await vscode.window.showErrorMessage(
      `StirPHP: could not start the language server (\`${path} lsp\`): ${error}`,
      'Open Settings',
    );
    if (choice === 'Open Settings') {
      await vscode.commands.executeCommand('workbench.action.openSettings', 'sphp.server.path');
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

/** @param {vscode.ExtensionContext} context */
export async function activate(context) {
  context.subscriptions.push(
    vscode.commands.registerCommand('sphp.restartServer', async () => {
      await stop();
      await start();
    }),
    vscode.workspace.onDidChangeConfiguration(async (e) => {
      if (e.affectsConfiguration('sphp.server') || e.affectsConfiguration('sphp.php')) {
        await stop();
        await start();
      }
    }),
  );
  await start();
}

export function deactivate() {
  return stop();
}
