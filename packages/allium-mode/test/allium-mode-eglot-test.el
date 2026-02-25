;;; allium-mode-eglot-test.el --- eglot integration tests for allium-mode -*- lexical-binding: t; -*-

;;; Commentary:

;; Tests registration behavior against eglot without requiring the package.

;;; Code:

(require 'ert)
(require 'allium-mode-test-helpers)

(defvar eglot-server-programs nil)

(ert-deftest allium-mode-registers-eglot-server-program ()
  "Loading allium-mode with eglot present adds the server command mapping."
  (allium-test-reset-environment)
  (setq eglot-server-programs nil)
  (provide 'eglot)
  (unwind-protect
      (progn
        (allium-test-load-mode t)
        (should (equal (alist-get 'allium-mode eglot-server-programs)
                       '("allium-lsp" "--stdio")))
        (should (equal (alist-get 'allium-ts-mode eglot-server-programs)
                       '("allium-lsp" "--stdio"))))
    (allium-test-reset-environment)))

(ert-deftest allium-mode-eglot-registration-honors-custom-server-command-at-load-time ()
  "Custom `allium-lsp-server-command` should be used when mode is loaded."
  (allium-test-reset-environment)
  (setq eglot-server-programs nil)
  (provide 'eglot)
  (unwind-protect
      (progn
        (setq allium-lsp-server-command '("node" "/tmp/allium-lsp.js" "--stdio"))
        (allium-test-load-mode t)
        (should (equal (alist-get 'allium-mode eglot-server-programs)
                       '("node" "/tmp/allium-lsp.js" "--stdio"))))
    (allium-test-reset-environment)))

(provide 'allium-mode-eglot-test)
;;; allium-mode-eglot-test.el ends here
