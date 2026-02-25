;;; allium-mode-test-runner.el --- Batch ERT runner for allium-mode -*- lexical-binding: t; -*-

;;; Commentary:

;; Runs all allium-mode ERT suites in batch mode.

;;; Code:

(require 'ert)

(load (expand-file-name "allium-mode-test-helpers.el" (file-name-directory (or load-file-name buffer-file-name))) nil 'nomessage)
(load (expand-file-name "allium-mode-core-test.el" (file-name-directory (or load-file-name buffer-file-name))) nil 'nomessage)
(load (expand-file-name "allium-mode-eglot-test.el" (file-name-directory (or load-file-name buffer-file-name))) nil 'nomessage)
(load (expand-file-name "allium-mode-lsp-mode-test.el" (file-name-directory (or load-file-name buffer-file-name))) nil 'nomessage)

(ert-run-tests-batch-and-exit t)

;;; allium-mode-test-runner.el ends here
