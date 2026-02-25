;;; emacs-test-install.el --- Install Emacs integration-test deps locally -*- lexical-binding: t; -*-

;;; Commentary:

;; Installs package dependencies into repo-local .emacs-test/elpa.

;;; Code:

(require 'emacs-test-bootstrap)
(require 'package)

(defun allium-emacs-test--ensure-package (pkg)
  "Install PKG unless already available."
  (unless (package-installed-p pkg)
    (package-install pkg)))

(let* ((needs-lsp-mode (not (package-installed-p 'lsp-mode)))
       ;; Eglot is built-in in modern Emacs; install package only when missing.
       (needs-eglot-package (and (not (fboundp 'eglot-ensure))
                                 (not (package-installed-p 'eglot)))))
  (when (or needs-lsp-mode needs-eglot-package)
    (package-refresh-contents))
  (when needs-lsp-mode
    (allium-emacs-test--ensure-package 'lsp-mode))
  (when needs-eglot-package
    (allium-emacs-test--ensure-package 'eglot)))

(princ (format "Installed test packages in %s\n" package-user-dir))

;;; emacs-test-install.el ends here
