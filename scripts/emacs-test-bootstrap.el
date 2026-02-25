;;; emacs-test-bootstrap.el --- Repo-local package bootstrap for Emacs tests -*- lexical-binding: t; -*-

;;; Commentary:

;; Configures package.el to use a repository-local, disposable test home.

;;; Code:

(let* ((root (expand-file-name ".." (file-name-directory (or load-file-name buffer-file-name))))
       (test-home (expand-file-name ".emacs-test/" root)))
  (setq user-emacs-directory test-home)
  (make-directory user-emacs-directory t)

  (require 'package)
  (setq package-user-dir (expand-file-name "elpa" user-emacs-directory))
  (setq package-archives
        '(("gnu" . "https://elpa.gnu.org/packages/")
          ("nongnu" . "https://elpa.nongnu.org/nongnu/")
          ("melpa" . "https://melpa.org/packages/")))
  (setq package-archive-priorities
        '(("gnu" . 20)
          ("nongnu" . 15)
          ("melpa" . 10)))
  (package-initialize))

(provide 'emacs-test-bootstrap)
;;; emacs-test-bootstrap.el ends here
