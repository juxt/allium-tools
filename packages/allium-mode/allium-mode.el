;;; allium-mode.el --- Major mode for Allium specifications  -*- lexical-binding: t; -*-

;; Version: 0.2.0
;; Author: JUXT
;; Keywords: languages, allium
;; Package-Requires: ((emacs "28.1"))

;;; Commentary:

;; This package provides a major mode for the Allium specification language.

;;; Code:

(defgroup allium nil
  "Support for the Allium specification language."
  :group 'languages)

(defcustom allium-indent-offset 4
  "Indentation offset for Allium blocks."
  :type 'integer
  :group 'allium)

(defcustom allium-lsp-server-command '("allium-lsp" "--stdio")
  "Command to start the Allium Language Server."
  :type '(repeat string)
  :group 'allium)

(defvar allium-mode-syntax-table
  (let ((st (make-syntax-table)))
    ;; Comments: -- to end of line
    (modify-syntax-entry ?- ". 12b" st)
    (modify-syntax-entry ?\n "> b" st)
    ;; Strings: "..."
    (modify-syntax-entry ?\" "\"" st)
    st)
  "Syntax table for `allium-mode'.")

(defvar allium-font-lock-keywords
  (let* ((keywords '("module" "use" "as" "rule" "entity" "external" "value" "enum"
                     "context" "config" "surface" "actor" "default" "variant"
                     "let" "not" "and" "or" "contract" "invariant" "implies"))
         (keyword-regexp (regexp-opt keywords 'symbols))
         (clause-keywords '("when" "requires" "ensures" "trigger" "provides" "tags"
                            "guidance" "becomes" "related" "exposes"
                            "identified_by" "contracts" "demands" "fulfils"
                            "guarantee" "timeout" "within" "transitions_to"
                            "facing"))
         (clause-regexp (concat "\\_<" (regexp-opt clause-keywords) ":")))
    `((,keyword-regexp . font-lock-keyword-face)
      (,clause-regexp . font-lock-keyword-face)
      ("\\_<\\(true\\|false\\|null\\)\\_>" . font-lock-constant-face)
      ("\\_<[0-9]+\\(\\.[0-9]+\\)?\\(?:\\.\\(?:seconds\\|minutes\\|hours\\|days\\)\\)?\\_>" . font-lock-constant-face)
      ;; Declarations: kind Name
      (,(concat "\\_<" (regexp-opt '("rule" "entity" "value" "enum" "surface" "actor" "variant" "contract" "invariant") 'symbols)
                "\\s-+\\([A-Za-z_][A-Za-z0-9_]*\\)")
       2 font-lock-type-face)
      ;; Field assignments: key:
      ("\\([A-Za-z_][A-Za-z0-9_]*\\):" 1 font-lock-variable-name-face)
      ;; Annotations: @invariant, @guidance, @guarantee
      ("@\\(invariant\\|guidance\\|guarantee\\)\\b" . font-lock-keyword-face)))
  "Font lock keywords for `allium-mode'.")

(defun allium-indent-line ()
  "Indent current line of Allium code."
  (interactive)
  (let ((savep (point-at-eol))
        (indent (condition-case nil
                    (save-excursion
                      (back-to-indentation)
                      (if (bobp) 0
                        (let* ((closing-brace-line-p (looking-at "^\\s-*}"))
                               (prev-indent (progn (forward-line -1) (current-indentation)))
                               (prev-opens-block-p (save-excursion
                                                     (end-of-line)
                                                     (re-search-backward "{\\s-*$" (line-beginning-position) t))))
                          (cond
                           (closing-brace-line-p
                            (max 0 (- prev-indent allium-indent-offset)))
                           (prev-opens-block-p
                            (+ prev-indent allium-indent-offset))
                           (t prev-indent)))))
                  (error 0))))
    (indent-line-to indent)
    (when (< (point) savep)
      (goto-char savep))))

;; --- Tree-sitter Support (Emacs 29+) ---

(declare-function treesit-parser-create "treesit.c")
(declare-function treesit-node-child-by-field-name "treesit.c")
(declare-function treesit-node-text "treesit.c")
(declare-function treesit-node-type "treesit.c")

(defvar allium--treesit-font-lock-rules
  (when (fboundp 'treesit-font-lock-rules)
    (treesit-font-lock-rules
     :language 'allium
     :feature 'comment
     '((comment) @font-lock-comment-face)

     :language 'allium
     :feature 'keyword
     '([
        "module" "use" "as" "rule" "entity" "external" "value" "enum"
        "context" "config" "surface" "actor" "default" "variant"
        "let" "not" "and" "or" "contract" "invariant" "implies"
       ] @font-lock-keyword-face
       (clause_keyword) @font-lock-keyword-face
       (annotation_keyword) @font-lock-keyword-face)

     :language 'allium
     :feature 'definition
     '((rule_declaration name: (identifier) @font-lock-type-face)
       (entity_declaration name: (identifier) @font-lock-type-face)
       (external_entity_declaration name: (identifier) @font-lock-type-face)
       (value_declaration name: (identifier) @font-lock-type-face)
       (enum_declaration name: (identifier) @font-lock-type-face)
       (surface_declaration name: (identifier) @font-lock-type-face)
       (actor_declaration name: (identifier) @font-lock-type-face)
       (default_declaration type: (identifier) @font-lock-type-face)
       (variant_declaration name: (identifier) @font-lock-type-face)
       (contract_declaration name: (identifier) @font-lock-type-face)
       (invariant_declaration name: (identifier) @font-lock-type-face))

     :language 'allium
     :feature 'variable
     '((field_assignment key: (identifier) @font-lock-variable-name-face)
       (let_binding name: (identifier) @font-lock-variable-name-face)
       (named_argument name: (identifier) @font-lock-variable-name-face))

     :language 'allium
     :feature 'function
     '((call_expression
        function: (identifier) @font-lock-function-name-face)
       (call_expression
        function: (member_expression
                   property: (identifier) @font-lock-function-name-face)))

     :language 'allium
     :feature 'string
     '((string_literal) @font-lock-string-face
       (string_interpolation
        "{" @font-lock-punctuation-face
        (identifier) @font-lock-variable-name-face
        "}" @font-lock-punctuation-face))

     :language 'allium
     :feature 'constant
     '((boolean_literal) @font-lock-constant-face
       (null_literal) @font-lock-constant-face
       (number_literal) @font-lock-constant-face
       (duration_literal) @font-lock-constant-face)

     :language 'allium
     :feature 'operator
     '([
        "=" "==" "!=" "<" ">" "<=" ">=" "=>" "->"
        "+" "-" "*" "/" "|"
       ] @font-lock-warning-face)

     :language 'allium
     :feature 'punctuation
     '([ "(" ")" "{" "}" ":" "," "." "@" ] @font-lock-punctuation-face))))

(defvar allium--treesit-defun-type-regexp
  (rx (or "rule_declaration"
          "entity_declaration"
          "external_entity_declaration"
          "value_declaration"
          "enum_declaration"
          "surface_declaration"
          "actor_declaration"
          "context_block"
          "config_block"
          "default_declaration"
          "variant_declaration"
          "contract_declaration"
          "invariant_declaration")))

(defun allium--treesit-defun-name (node)
  "Return the name of the defun NODE."
  (pcase (treesit-node-type node)
    ((or "rule_declaration" "entity_declaration" "external_entity_declaration"
         "value_declaration" "enum_declaration" "surface_declaration"
         "actor_declaration" "variant_declaration"
         "contract_declaration" "invariant_declaration")
     (treesit-node-text (treesit-node-child-by-field-name node "name") t))
    ("default_declaration"
     (treesit-node-text (treesit-node-child-by-field-name node "name") t))
    ("context_block" "context")
    ("config_block" "config")))

(defvar allium--treesit-imenu-settings
  '(("Rule" "\\`rule_declaration\\'" nil nil)
    ("Entity" "\\`e\\(?:ntity\\|xternal_entity\\)_declaration\\'" nil nil)
    ("Value" "\\`value_declaration\\'" nil nil)
    ("Enum" "\\`enum_declaration\\'" nil nil)
    ("Config" "\\`config_block\\'" nil nil)
    ("Context" "\\`context_block\\'" nil nil)
    ("Contract" "\\`contract_declaration\\'" nil nil)
    ("Invariant" "\\`invariant_declaration\\'" nil nil)))

;;;###autoload
(define-derived-mode allium-mode prog-mode "Allium"
  "Major mode for editing Allium specifications."
  :syntax-table allium-mode-syntax-table
  (setq-local comment-start "-- ")
  (setq-local comment-end "")
  (setq-local font-lock-defaults '(allium-font-lock-keywords))
  (setq-local indent-line-function 'allium-indent-line))

;;;###autoload
(define-derived-mode allium-ts-mode allium-mode "Allium[TS]"
  "Major mode for editing Allium specifications using tree-sitter."
  :syntax-table allium-mode-syntax-table
  (when (and (fboundp 'treesit-parser-create)
             (condition-case nil
                 (progn
                   (treesit-parser-create 'allium)
                   t)
               (error nil)))
    (setq-local treesit-font-lock-settings allium--treesit-font-lock-rules)
    (setq-local treesit-font-lock-feature-list
                '((comment definition)
                  (keyword variable function)
                  (string constant operator)
                  (punctuation)))
    (setq-local treesit-defun-type-regexp allium--treesit-defun-type-regexp)
    (setq-local treesit-defun-name-function #'allium--treesit-defun-name)
    (setq-local treesit-simple-imenu-settings allium--treesit-imenu-settings)
    (when (fboundp 'treesit-major-mode-setup)
      (treesit-major-mode-setup))))

;;;###autoload
(add-to-list 'auto-mode-alist '("\\.allium\\'" . allium-mode))

(with-eval-after-load 'eglot
  (add-to-list 'eglot-server-programs
               `(allium-mode . ,allium-lsp-server-command))
  (add-to-list 'eglot-server-programs
               `(allium-ts-mode . ,allium-lsp-server-command)))

(with-eval-after-load 'lsp-mode
  (add-to-list 'lsp-language-id-configuration '(allium-mode . "allium"))
  (add-to-list 'lsp-language-id-configuration '(allium-ts-mode . "allium"))
  (lsp-register-client
   (make-lsp-client :new-connection (lsp-stdio-connection (lambda () allium-lsp-server-command))
                    :major-modes '(allium-mode allium-ts-mode)
                    :priority 0
                    :server-id 'allium-lsp
                    :language-id "allium")))

(provide 'allium-mode)
;;; allium-mode.el ends here
