;;; allium-mode-lsp-mode-test.el --- lsp-mode integration tests for allium-mode -*- lexical-binding: t; -*-

;;; Commentary:

;; Tests lsp-mode client registration with local stubs for speed and stability.

;;; Code:

(require 'ert)
(require 'cl-lib)
(require 'allium-mode-test-helpers)

(defvar lsp-language-id-configuration nil)

(ert-deftest allium-mode-registers-lsp-client-with-expected-settings ()
  "Loading allium-mode with lsp-mode present registers allium-lsp client data."
  (allium-test-reset-environment)
  (let (registered-client)
    (cl-letf (((symbol-function 'lsp-register-client)
               (lambda (client) (setq registered-client client)))
              ((symbol-function 'make-lsp-client)
               (lambda (&rest plist) plist))
              ((symbol-function 'lsp-stdio-connection)
               (lambda (command-fn) (funcall command-fn))))
      (provide 'lsp-mode)
      (unwind-protect
          (progn
            (allium-test-load-mode t)
            (should registered-client)
            (should (equal (plist-get registered-client :major-modes)
                           '(allium-mode allium-ts-mode)))
            (should (equal (plist-get registered-client :new-connection)
                           '("allium-lsp" "--stdio")))
            (should (eq (plist-get registered-client :server-id) 'allium-lsp))
            (should (equal (plist-get registered-client :language-id) "allium"))
            (should (equal (plist-get registered-client :priority) 0))
            (should (equal (alist-get 'allium-mode lsp-language-id-configuration) "allium"))
            (should (equal (alist-get 'allium-ts-mode lsp-language-id-configuration) "allium")))
        (allium-test-reset-environment)))))

(ert-deftest allium-mode-lsp-registration-honors-custom-server-command-at-load-time ()
  "Custom `allium-lsp-server-command` should be used by lsp-mode registration."
  (allium-test-reset-environment)
  (let (registered-client)
    (cl-letf (((symbol-function 'lsp-register-client)
               (lambda (client) (setq registered-client client)))
              ((symbol-function 'make-lsp-client)
               (lambda (&rest plist) plist))
              ((symbol-function 'lsp-stdio-connection)
               (lambda (command-fn) (funcall command-fn))))
      (provide 'lsp-mode)
      (unwind-protect
          (progn
            (setq allium-lsp-server-command '("node" "/tmp/allium-lsp.js" "--stdio"))
            (allium-test-load-mode t)
            (should registered-client)
            (should (equal (plist-get registered-client :new-connection)
                           '("node" "/tmp/allium-lsp.js" "--stdio"))))
        (allium-test-reset-environment)))))

(provide 'allium-mode-lsp-mode-test)
;;; allium-mode-lsp-mode-test.el ends here
