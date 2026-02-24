;;; allium-mode-test-helpers.el --- Shared helpers for allium-mode tests -*- lexical-binding: t; -*-

;;; Commentary:

;; Test helpers to load and reset allium-mode in batch ERT runs.

;;; Code:

(require 'cl-lib)

(defconst allium-test--package-dir
  (expand-file-name ".." (file-name-directory (or load-file-name buffer-file-name)))
  "Absolute path to the allium-mode package directory.")

(defconst allium-test--repo-root
  (expand-file-name "../.." allium-test--package-dir)
  "Absolute path to the monorepo root.")

(defconst allium-test--mode-file
  (expand-file-name "allium-mode.el" allium-test--package-dir)
  "Absolute path to allium-mode.el.")

(defconst allium-test--lsp-bin
  (expand-file-name "packages/allium-lsp/dist/bin.js" allium-test--repo-root)
  "Absolute path to the built allium-lsp Node entrypoint.")

(defconst allium-test--treesit-lib-dir
  (expand-file-name ".emacs-test/tree-sitter" allium-test--repo-root)
  "Directory containing locally built tree-sitter grammars for tests.")

(defun allium-test-configure-treesit-load-path ()
  "Ensure Emacs can discover locally built tree-sitter grammars."
  (when (file-directory-p allium-test--treesit-lib-dir)
    (add-to-list 'treesit-extra-load-path allium-test--treesit-lib-dir)))

(defun allium-test-unload-feature-if-loaded (feature)
  "Unload FEATURE when loaded; ignore errors to keep tests isolated."
  (when (featurep feature)
    (ignore-errors (unload-feature feature t)))
  ;; Synthetic test features (provided without a backing file) can remain in
  ;; `features` even when unload fails; remove them explicitly for isolation.
  (setq features (delq feature features)))

(defun allium-test-load-mode (&optional preserve-client-features)
  "Load allium-mode.el from source after unloading previous copies.
When PRESERVE-CLIENT-FEATURES is non-nil, keep eglot/lsp-mode loaded."
  (allium-test-configure-treesit-load-path)
  (allium-test-unload-feature-if-loaded 'allium-mode)
  (unless preserve-client-features
    (allium-test-unload-feature-if-loaded 'eglot)
    (allium-test-unload-feature-if-loaded 'lsp-mode))
  (load allium-test--mode-file nil 'nomessage))

(defun allium-test-reset-environment ()
  "Reset feature state touched by allium-mode tests."
  (allium-test-unload-feature-if-loaded 'allium-mode)
  (allium-test-unload-feature-if-loaded 'eglot)
  (allium-test-unload-feature-if-loaded 'lsp-mode))

(provide 'allium-mode-test-helpers)
;;; allium-mode-test-helpers.el ends here
