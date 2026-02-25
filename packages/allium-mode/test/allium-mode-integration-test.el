;;; allium-mode-integration-test.el --- Integration tests for allium-mode -*- lexical-binding: t; -*-

;;; Commentary:

;; Real-client integration tests that exercise a live allium-lsp process.

;;; Code:

(require 'ert)
(require 'allium-mode-test-helpers)
(require 'subr-x)
(require 'xref)
(require 'seq)

(defvar eglot-server-programs nil)

(defun allium-test--wait-until (predicate timeout-seconds)
  "Wait until PREDICATE is non-nil or TIMEOUT-SECONDS elapse."
  (let* ((deadline (+ (float-time) timeout-seconds))
         value)
    (while (and (< (float-time) deadline)
                (not (setq value (funcall predicate))))
      (accept-process-output nil 0.05))
    value))

(defun allium-test--with-temp-allium-file (contents fn)
  "Create a temp .allium file with CONTENTS and call FN with its path."
  (let* ((root (make-temp-file "allium-emacs-it-" t))
         (file (expand-file-name "spec.allium" root)))
    (unwind-protect
        (progn
          (make-directory (expand-file-name ".git" root) t)
          (with-temp-file file
            (insert contents))
          (funcall fn file))
      (delete-directory root t))))

(defun allium-test--with-temp-allium-project (files fn)
  "Create a temp project from FILES alist and call FN with project root."
  (let ((root (make-temp-file "allium-emacs-project-" t)))
    (unwind-protect
        (progn
          (make-directory (expand-file-name ".git" root) t)
          (dolist (entry files)
            (let* ((rel (car entry))
                   (contents (cdr entry))
                   (target (expand-file-name rel root)))
              (make-directory (file-name-directory target) t)
              (with-temp-file target
                (insert contents))))
          (funcall fn root))
      (delete-directory root t))))

(defun allium-test--with-eglot-managed-buffer (contents fn)
  "Open CONTENTS in a temp allium file, connect eglot, then run FN in buffer."
  (unless (require 'eglot nil t)
    (ert-skip "eglot is unavailable in this Emacs build"))
  (unless (file-exists-p allium-test--lsp-bin)
    (ert-skip "allium-lsp binary is missing; run package build first"))

  (allium-test-load-mode t)
  (let ((server-command (list "node" allium-test--lsp-bin "--stdio")))
    (setf (alist-get 'allium-mode eglot-server-programs) server-command)
    (allium-test--with-temp-allium-file
     contents
     (lambda (file)
       (let ((buf (find-file-noselect file)))
         (unwind-protect
             (with-current-buffer buf
               (allium-mode)
               (eglot-ensure)
               (run-hooks 'post-command-hook)
               (should (allium-test--wait-until #'eglot-current-server 10))
               (funcall fn file))
           (when (buffer-live-p buf)
             (with-current-buffer buf
               (when (fboundp 'eglot-shutdown)
                 (ignore-errors (eglot-shutdown (eglot-current-server))))
               (set-buffer-modified-p nil))
             (kill-buffer buf))))))))

(ert-deftest allium-mode-eglot-integration-connects-to-real-server ()
  "eglot starts a real allium-lsp process for an allium buffer."
  (allium-test--with-eglot-managed-buffer
   "rule Ping {\n  when: Trigger()\n  ensures: Done()\n}\n"
   (lambda (_file)
     (should (eglot-managed-p)))))

(ert-deftest allium-ts-mode-eglot-integration-connects-to-real-server ()
  "eglot should also manage `allium-ts-mode` buffers."
  (unless (require 'eglot nil t)
    (ert-skip "eglot is unavailable in this Emacs build"))
  (unless (file-exists-p allium-test--lsp-bin)
    (ert-skip "allium-lsp binary is missing; run package build first"))
  (allium-test-load-mode t)
  (let ((server-command (list "node" allium-test--lsp-bin "--stdio")))
    (setf (alist-get 'allium-ts-mode eglot-server-programs) server-command)
    (allium-test--with-temp-allium-file
     "rule Ping {\n  when: Trigger()\n  ensures: Done()\n}\n"
     (lambda (file)
       (let ((buf (find-file-noselect file)))
         (unwind-protect
             (with-current-buffer buf
               (allium-ts-mode)
               (eglot-ensure)
               (run-hooks 'post-command-hook)
               (should (allium-test--wait-until #'eglot-current-server 10))
               (should (eglot-managed-p)))
           (when (buffer-live-p buf)
             (with-current-buffer buf
               (when (fboundp 'eglot-shutdown)
                 (ignore-errors (eglot-shutdown (eglot-current-server))))
               (set-buffer-modified-p nil))
             (kill-buffer buf))))))))

(ert-deftest allium-mode-lsp-mode-integration-connects-to-real-server ()
  "lsp-mode starts a real allium-lsp process for an allium buffer when available."
  (unless (require 'lsp-mode nil t)
    (ert-skip "lsp-mode is not installed"))
  (unless (file-exists-p allium-test--lsp-bin)
    (ert-skip "allium-lsp binary is missing; run package build first"))

  (allium-test-load-mode t)
  (let ((allium-lsp-server-command (list "node" allium-test--lsp-bin "--stdio"))
        (lsp-auto-guess-root t)
        (lsp-enable-snippet nil))
    (allium-test--with-temp-allium-file
     "rule Ping {\n  when: Trigger()\n  ensures: Done()\n}\n"
     (lambda (file)
       (let ((buf (find-file-noselect file)))
         (unwind-protect
             (with-current-buffer buf
               (allium-mode)
               (lsp)
               (should (allium-test--wait-until (lambda () (bound-and-true-p lsp-mode)) 10))
               (should (allium-test--wait-until #'lsp-workspaces 10)))
           (when (buffer-live-p buf)
             (with-current-buffer buf
               (ignore-errors (lsp-disconnect))
               (set-buffer-modified-p nil))
             (kill-buffer buf))))))))

(ert-deftest allium-mode-lsp-mode-integration-returns-hover-result ()
  "lsp-mode should return hover content from a live allium-lsp session."
  (unless (require 'lsp-mode nil t)
    (ert-skip "lsp-mode is not installed"))
  (unless (file-exists-p allium-test--lsp-bin)
    (ert-skip "allium-lsp binary is missing; run package build first"))
  (allium-test-load-mode t)
  (let ((allium-lsp-server-command (list "node" allium-test--lsp-bin "--stdio"))
        (lsp-auto-guess-root t)
        (lsp-enable-snippet nil))
    (allium-test--with-temp-allium-file
     "rule Ping {\n  when: Trigger()\n  ensures: Done()\n}\n"
     (lambda (_file)
       (let ((buf (find-file-noselect _file)))
         (unwind-protect
             (with-current-buffer buf
               (allium-mode)
               (lsp)
               (should (allium-test--wait-until (lambda () (bound-and-true-p lsp-mode)) 10))
               (should (allium-test--wait-until #'lsp-workspaces 10))
               (should (allium-test--wait-until
                        (lambda ()
                          (let ((workspace (car (lsp-workspaces))))
                            (and workspace
                                 (eq (lsp--workspace-status workspace) 'initialized))))
                        10))
               (unless (lsp-feature? "textDocument/hover")
                 (ert-skip "lsp-mode reports hover unsupported in this session"))
               (let ((hover (lsp-request
                             "textDocument/hover"
                             `(:textDocument (:uri ,(lsp--buffer-uri))
                               :position (:line 2 :character 6)))))
                 (should hover)))
           (when (buffer-live-p buf)
             (with-current-buffer buf
               (ignore-errors (lsp-disconnect))
               (set-buffer-modified-p nil))
             (kill-buffer buf))))))))

(ert-deftest allium-mode-eglot-integration-returns-hover-result ()
  "eglot should return hover content from a live allium-lsp session."
  (allium-test--with-eglot-managed-buffer
   "rule Ping {\n  when: Trigger()\n  ensures: Done()\n}\n"
   (lambda (file)
     (let* ((server (eglot-current-server))
            (hover (jsonrpc-request
                    server
                    :textDocument/hover
                    `(:textDocument (:uri ,(eglot--path-to-uri file))
                      :position (:line 2 :character 6)))))
       (should hover)))))

(ert-deftest allium-mode-eglot-command-format-buffer ()
  "eglot-format-buffer should apply formatter edits in-place."
  (allium-test--with-eglot-managed-buffer
   "rule Ping {\nwhen: Trigger()\nensures: Done()\n}\n"
   (lambda (_file)
     (should (string-match-p "^when:"
                             (save-excursion
                               (goto-char (point-min))
                               (forward-line 1)
                               (buffer-substring-no-properties
                                (line-beginning-position)
                                (line-end-position)))))
     (eglot-format-buffer)
     (should (allium-test--wait-until
              (lambda ()
                (string-match-p "^    when:"
                                (save-excursion
                                  (goto-char (point-min))
                                  (forward-line 1)
                                  (buffer-substring-no-properties
                                   (line-beginning-position)
                                   (line-end-position)))))
              10)))))

(ert-deftest allium-mode-eglot-command-xref-find-definitions ()
  "xref-find-definitions should jump to the symbol declaration."
  (allium-test--with-eglot-managed-buffer
   (concat
    "rule TriggerA {\n  when: Trigger()\n  ensures: Done()\n}\n\n"
    "rule UsesTriggerA {\n  when: TriggerA()\n  ensures: Done()\n}\n")
   (lambda (_file)
     (goto-char (point-min))
     (search-forward "when: TriggerA()")
     (search-backward "TriggerA")
     (xref-find-definitions "TriggerA")
     (should (looking-at "TriggerA")))))

(ert-deftest allium-mode-eglot-command-xref-find-references ()
  "References request should resolve local references."
  (allium-test--with-eglot-managed-buffer
   (concat
    "rule TriggerA {\n  when: Trigger()\n  ensures: Done()\n}\n\n"
    "rule UsesTriggerA {\n  when: TriggerA()\n  ensures: Done()\n}\n")
   (lambda (file)
     (let* ((server (eglot-current-server))
            (references (jsonrpc-request
                         server
                         :textDocument/references
                         `(:textDocument (:uri ,(eglot--path-to-uri file))
                           :position (:line 0 :character 6)
                           :context (:includeDeclaration t)))))
       (should (sequencep references))
       (should (>= (length references) 2))))))

(ert-deftest allium-mode-eglot-code-actions-include-fix-all ()
  "Code actions should include the source fix-all action for fixable files."
  (allium-test--with-eglot-managed-buffer
   "rule Broken {\n  when: Trigger()\n}\n"
   (lambda (file)
     (let* ((server (eglot-current-server))
            (actions (jsonrpc-request
                      server
                      :textDocument/codeAction
                      `(:textDocument (:uri ,(eglot--path-to-uri file))
                        :range (:start (:line 0 :character 0)
                                :end (:line 0 :character 0))
                        :context (:diagnostics [])))))
       (should (sequencep actions))
       (should (seq-some
                (lambda (action)
                  (equal (plist-get action :title) "Allium: Apply All Safe Fixes"))
                actions))))))

(ert-deftest allium-mode-eglot-command-rename ()
  "eglot-rename should update symbol declaration and references."
  (allium-test--with-eglot-managed-buffer
   (concat
    "rule TriggerA {\n  when: Trigger()\n  ensures: Done()\n}\n\n"
    "rule UsesTriggerA {\n  when: TriggerA()\n  ensures: Done()\n}\n")
   (lambda (_file)
     (goto-char (point-min))
     (search-forward "rule TriggerA")
     (search-backward "TriggerA")
     (eglot-rename "TriggerRenamed")
     (should (save-excursion
               (goto-char (point-min))
               (search-forward "rule TriggerRenamed" nil t))))))

(ert-deftest allium-mode-eglot-multi-file-definition-via-use-alias ()
  "Definition lookup should cross files through `use` alias references."
  (unless (require 'eglot nil t)
    (ert-skip "eglot is unavailable in this Emacs build"))
  (unless (file-exists-p allium-test--lsp-bin)
    (ert-skip "allium-lsp binary is missing; run package build first"))
  (allium-test-load-mode t)
  (let ((server-command (list "node" allium-test--lsp-bin "--stdio")))
    (setf (alist-get 'allium-mode eglot-server-programs) server-command)
    (allium-test--with-temp-allium-project
     '(("defs.allium" . "rule TriggerA {\n  when: Trigger()\n  ensures: Done()\n}\n")
       ("main.allium" . "use \"./defs\" as defs\n\nrule Uses {\n  when: defs.TriggerA()\n  ensures: Done()\n}\n"))
     (lambda (root)
       (let* ((main-file (expand-file-name "main.allium" root))
              (defs-file (expand-file-name "defs.allium" root))
              (buf (find-file-noselect main-file)))
         (unwind-protect
             (with-current-buffer buf
               (allium-mode)
               (eglot-ensure)
               (run-hooks 'post-command-hook)
               (should (allium-test--wait-until #'eglot-current-server 10))
               (goto-char (point-min))
               (search-forward "defs.TriggerA")
               (search-backward "TriggerA")
               (xref-find-definitions "TriggerA")
               (should (string= (buffer-file-name) defs-file)))
           (when (buffer-live-p buf)
             (with-current-buffer buf
               (when (fboundp 'eglot-shutdown)
                 (ignore-errors (eglot-shutdown (eglot-current-server))))
               (set-buffer-modified-p nil))
             (kill-buffer buf))))))))

(provide 'allium-mode-integration-test)
;;; allium-mode-integration-test.el ends here
