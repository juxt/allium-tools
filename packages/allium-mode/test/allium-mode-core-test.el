;;; allium-mode-core-test.el --- Core tests for allium-mode -*- lexical-binding: t; -*-

;;; Commentary:

;; Unit tests for core allium-mode behavior that does not require an LSP client.

;;; Code:

(require 'ert)
(require 'cl-lib)
(require 'imenu)
(require 'allium-mode-test-helpers)

(ert-deftest allium-mode-sets-core-buffer-locals ()
  "allium-mode configures expected editor-local defaults."
  (allium-test-load-mode)
  (with-temp-buffer
    (allium-mode)
    (should (eq major-mode 'allium-mode))
    (should (equal comment-start "-- "))
    (should (equal comment-end ""))
    (should (eq indent-line-function #'allium-indent-line))
    (should (equal font-lock-defaults '(allium-font-lock-keywords)))
    (should (equal allium-indent-offset 4))))

(ert-deftest allium-mode-registers-file-extension ()
  "\.allium files are mapped to allium-mode."
  (allium-test-load-mode)
  (let ((mode (assoc-default "sample.allium" auto-mode-alist #'string-match)))
    (should (eq mode 'allium-mode))))

(ert-deftest allium-mode-indents-block-content-and-closing-braces ()
  "Indentation follows block structure around braces."
  (allium-test-load-mode)
  (with-temp-buffer
    (insert "rule A {\nwhen: Trigger()\nensures: Done()\n}\n")
    (allium-mode)
    (indent-region (point-min) (point-max))
    (should (equal (buffer-string)
                   (concat
                    "rule A {\n"
                    "    when: Trigger()\n"
                    "    ensures: Done()\n"
                    "}\n")))))

(ert-deftest allium-mode-indentation-respects-custom-indent-offset ()
  "Indentation should use `allium-indent-offset` when opening/closing blocks."
  (allium-test-load-mode)
  (with-temp-buffer
    (insert "rule A {\nwhen: Trigger()\n}\n")
    (allium-mode)
    (let ((allium-indent-offset 2))
      (indent-region (point-min) (point-max)))
    (should (equal (buffer-string)
                   (concat
                    "rule A {\n"
                    "  when: Trigger()\n"
                    "}\n")))))

(ert-deftest allium-mode-recognizes-line-comments-with-double-dash ()
  "Syntax table should treat `--` as a line comment delimiter."
  (allium-test-load-mode)
  (with-temp-buffer
    (insert "-- hello\nrule A {\n}\n")
    (allium-mode)
    (goto-char (point-min))
    (search-forward "hello")
    (should (nth 4 (syntax-ppss)))))

(ert-deftest allium-mode-applies-font-lock-faces-for-key-parts ()
  "Regex font-lock should highlight declarations, clauses, and field keys."
  (allium-test-load-mode)
  (with-temp-buffer
    (insert "rule Ping {\n  when: Trigger()\n  ensures: Done()\n}\n")
    (allium-mode)
    (font-lock-ensure)
    (goto-char (point-min))
    (search-forward "rule")
    (should (eq (get-text-property (1- (point)) 'face) 'font-lock-keyword-face))
    (search-forward "Ping")
    (should (eq (get-text-property (1- (point)) 'face) 'font-lock-type-face))
    (search-forward "when:")
    (should (eq (get-text-property (- (point) 2) 'face) 'font-lock-keyword-face))
    (search-forward "ensures:")
    (should (eq (get-text-property (- (point) 2) 'face) 'font-lock-keyword-face))))

(ert-deftest allium-ts-mode-is-selectable-without-grammar-install ()
  "allium-ts-mode should still activate even if grammar is unavailable."
  (allium-test-load-mode)
  (with-temp-buffer
    (allium-ts-mode)
    (should (eq major-mode 'allium-ts-mode))))

(ert-deftest allium-ts-mode-configures-treesit-when-grammar-is-ready ()
  "allium-ts-mode should configure tree-sitter locals when parser can be created."
  (allium-test-load-mode)
  (let (parser-language setup-called)
    (cl-letf (((symbol-function 'treesit-parser-create)
               (lambda (lang) (setq parser-language lang)))
              ((symbol-function 'treesit-major-mode-setup)
               (lambda () (setq setup-called t))))
      (with-temp-buffer
        (allium-ts-mode)
        (should (eq parser-language 'allium))
        (should (equal treesit-font-lock-settings allium--treesit-font-lock-rules))
        (should (equal treesit-defun-type-regexp allium--treesit-defun-type-regexp))
        (should (eq treesit-defun-name-function #'allium--treesit-defun-name))
        (should (equal treesit-simple-imenu-settings allium--treesit-imenu-settings))
        (should setup-called)))))

(ert-deftest allium-ts-mode-skips-treesit-setup-when-parser-creation-fails ()
  "allium-ts-mode should stay active even when parser creation signals an error."
  (allium-test-load-mode)
  (let (setup-called)
    (cl-letf (((symbol-function 'treesit-parser-create)
               (lambda (_lang) (error "parser unavailable")))
              ((symbol-function 'treesit-major-mode-setup)
               (lambda () (setq setup-called t))))
      (with-temp-buffer
        (allium-ts-mode)
        (should (eq major-mode 'allium-ts-mode))
        (should-not (local-variable-p 'treesit-font-lock-settings))
        (should-not setup-called)))))

(ert-deftest allium-ts-mode-imenu-lists-top-level-declarations-with-repo-grammar ()
  "With the repo grammar built, imenu should include top-level declaration names."
  (allium-test-load-mode)
  (unless (and (file-directory-p allium-test--treesit-lib-dir)
               (fboundp 'treesit-parser-create))
    (ert-skip "tree-sitter parser setup unavailable"))
  (unless (fboundp 'treesit-major-mode-setup)
    (ert-skip "treesit major-mode imenu wiring unavailable in this Emacs build"))
  (with-temp-buffer
    (insert "entity Ticket {\n  id: String\n}\n\nrule Close {\n  when: Trigger()\n  ensures: Done()\n}\n")
    (allium-ts-mode)
    (let ((index (imenu--make-index-alist t)))
      (should (string-match-p "Ticket" (format "%S" index)))
      (should (string-match-p "Close" (format "%S" index))))))

(ert-deftest allium-ts-mode-uses-real-tree-sitter-grammar-when-installed ()
  "When grammar artifacts exist, Emacs should create an allium parser from them."
  (allium-test-load-mode)
  (unless (file-directory-p allium-test--treesit-lib-dir)
    (ert-skip "local tree-sitter grammar directory is unavailable"))
  (unless (fboundp 'treesit-parser-create)
    (ert-skip "tree-sitter parser APIs are unavailable in this Emacs build"))
  (with-temp-buffer
    (insert "rule A {\n  when: Trigger()\n  ensures: Done()\n}\n")
    (allium-mode)
    (should-not (condition-case nil
                    (progn (treesit-parser-create 'allium) nil)
                  (error t)))
    (should (> (length (treesit-parser-list)) 0))))

(ert-deftest allium-treesit-defun-name-supports-context-and-config-nodes ()
  "allium--treesit-defun-name should map anonymous block node types to labels."
  (allium-test-load-mode)
  (cl-letf (((symbol-function 'treesit-node-type) (lambda (node) node)))
    (should (equal (allium--treesit-defun-name "context_block") "context"))
    (should (equal (allium--treesit-defun-name "config_block") "config"))))

(ert-deftest allium-treesit-defun-name-reads-declaration-name-field ()
  "allium--treesit-defun-name should read the name field for declarations."
  (allium-test-load-mode)
  (cl-letf (((symbol-function 'treesit-node-type) (lambda (_node) "default_declaration"))
            ((symbol-function 'treesit-node-child-by-field-name)
             (lambda (_node field-name) field-name))
            ((symbol-function 'treesit-node-text)
             (lambda (node _with-properties) (format "name-from-%s" node))))
    (should (equal (allium--treesit-defun-name 'fake-node) "name-from-name"))))

(provide 'allium-mode-core-test)
;;; allium-mode-core-test.el ends here
