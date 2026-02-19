"use strict";

const cp = require("node:child_process");

class LspHarness {
  constructor(modulePath, options = {}) {
    // Use Node IPC transport for deterministic integration tests. In some
    // non-interactive runners stdin can close early, and vscode-languageserver
    // exits with code 1 before initialize when running over --stdio.
    this.process = cp.fork(modulePath, ["--node-ipc"], {
      cwd: options.cwd,
      silent: true,
      env: options.env ?? process.env,
    });

    this.nextId = 1;
    this.pending = new Map();
    this.notifications = [];
    this.notificationWaiters = [];
    this.stderr = "";
    this.exited = false;

    this.process.stderr.on("data", (chunk) => {
      this.stderr += chunk.toString("utf8");
    });
    this.process.on("message", (message) => this.#onMessage(message));
    this.process.on("exit", () => {
      this.exited = true;
      for (const { reject, timer } of this.pending.values()) {
        clearTimeout(timer);
        reject(new Error(`LSP process exited. stderr:\n${this.stderr}`));
      }
      this.pending.clear();
      for (const waiter of this.notificationWaiters) {
        clearTimeout(waiter.timer);
        waiter.reject(new Error(`LSP process exited. stderr:\n${this.stderr}`));
      }
      this.notificationWaiters = [];
    });
  }

  async initialize(params) {
    const result = await this.request("initialize", params);
    this.notify("initialized", {});
    return result;
  }

  request(method, params, timeoutMs = 5000) {
    if (this.exited) {
      return Promise.reject(
        new Error(`LSP process already exited. stderr:\n${this.stderr}`),
      );
    }
    const id = this.nextId++;
    const payload = { jsonrpc: "2.0", id, method, params };
    this.#send(payload);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(
          new Error(
            `Timed out waiting for '${method}'. stderr:\n${this.stderr}`,
          ),
        );
      }, timeoutMs);
      this.pending.set(id, { resolve, reject, timer });
    });
  }

  notify(method, params) {
    if (this.exited) {
      return;
    }
    this.#send({ jsonrpc: "2.0", method, params });
  }

  waitForNotification(method, predicate = () => true, timeoutMs = 5000) {
    const queued = this.notifications.find(
      (entry) => entry.method === method && predicate(entry.params),
    );
    if (queued) {
      return Promise.resolve(queued.params);
    }

    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.notificationWaiters = this.notificationWaiters.filter(
          (w) => w.resolve !== resolve,
        );
        reject(
          new Error(
            `Timed out waiting for notification '${method}'. stderr:\n${this.stderr}`,
          ),
        );
      }, timeoutMs);

      this.notificationWaiters.push({
        method,
        predicate,
        resolve: (params) => {
          clearTimeout(timer);
          resolve(params);
        },
        reject,
        timer,
      });
    });
  }

  async shutdown() {
    if (this.exited) {
      return;
    }

    try {
      await this.request("shutdown", null, 2000);
    } catch {
      // Ignore shutdown errors and always try exit/kill.
    }
    this.notify("exit");

    await new Promise((resolve) => {
      const timer = setTimeout(() => {
        if (!this.exited) {
          this.process.kill("SIGKILL");
        }
        resolve();
      }, 1500);
      this.process.once("exit", () => {
        clearTimeout(timer);
        resolve();
      });
    });
  }

  #send(message) {
    this.process.send(message);
  }

  #onMessage(message) {
    if (Object.prototype.hasOwnProperty.call(message, "id")) {
      const pending = this.pending.get(message.id);
      if (!pending) {
        return;
      }
      this.pending.delete(message.id);
      clearTimeout(pending.timer);
      if (message.error) {
        const err = new Error(
          message.error.message || `JSON-RPC error ${message.error.code}`,
        );
        err.code = message.error.code;
        err.data = message.error.data;
        pending.reject(err);
      } else {
        pending.resolve(message.result);
      }
      return;
    }

    if (message.method) {
      this.notifications.push({
        method: message.method,
        params: message.params,
      });

      const remaining = [];
      for (const waiter of this.notificationWaiters) {
        if (
          waiter.method === message.method &&
          waiter.predicate(message.params)
        ) {
          waiter.resolve(message.params);
        } else {
          remaining.push(waiter);
        }
      }
      this.notificationWaiters = remaining;
    }
  }
}

module.exports = { LspHarness };
