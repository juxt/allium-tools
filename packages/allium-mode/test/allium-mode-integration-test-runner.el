;;; allium-mode-integration-test-runner.el --- Batch integration runner -*- lexical-binding: t; -*-

;;; Commentary:

;; Runs allium-mode integration ERT suites against real LSP clients/servers.

;;; Code:

(require 'ert)

(load (expand-file-name "allium-mode-test-helpers.el" (file-name-directory (or load-file-name buffer-file-name))) nil 'nomessage)
(load (expand-file-name "allium-mode-integration-test.el" (file-name-directory (or load-file-name buffer-file-name))) nil 'nomessage)

(ert-run-tests-batch-and-exit t)

;;; allium-mode-integration-test-runner.el ends here
