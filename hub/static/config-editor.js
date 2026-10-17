// The config file editor on `/settings/<file>`: Ctrl+S saves, Tab indents, and leaving
// the page with unsaved changes asks first.
(() => {
  const indent = "    ";
  // After Escape, Tab moves focus out of the text box as usual.
  let tabMovesFocus = false;

  const editor = () => document.getElementById("config-editor");

  // A form answered with an error still holds text that is not on disk.
  function unsaved(form) {
    const text = form?.elements.text;
    return !!text && (form.dataset.unsaved === "true" || text.value !== text.defaultValue);
  }

  function insert(textarea, text) {
    // execCommand keeps the browser's undo history; setRangeText is the fallback.
    if (!document.execCommand("insertText", false, text)) {
      textarea.setRangeText(text, textarea.selectionStart, textarea.selectionEnd, "end");
    }
  }

  // Indents or outdents every line the selection touches.
  function indentLines(textarea, outdent) {
    const { value, selectionStart, selectionEnd } = textarea;
    const start = value.lastIndexOf("\n", selectionStart - 1) + 1;
    const end = selectionEnd > selectionStart && value[selectionEnd - 1] === "\n"
      ? selectionEnd - 1
      : selectionEnd;
    const lines = value.slice(start, end).split("\n");
    const changed = lines
      .map((line) => (outdent ? line.replace(new RegExp(`^ {1,${indent.length}}`), "") : indent + line))
      .join("\n");
    textarea.setSelectionRange(start, end);
    insert(textarea, changed);
    textarea.setSelectionRange(start, start + changed.length);
  }

  document.addEventListener("keydown", (event) => {
    const form = editor();
    if (!form) return;
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
      event.preventDefault();
      form.requestSubmit();
      return;
    }
    const textarea = form.elements.text;
    if (event.target !== textarea) return;
    if (event.key === "Escape") {
      tabMovesFocus = true;
      return;
    }
    if (event.key !== "Tab" || event.ctrlKey || event.altKey || event.metaKey) {
      tabMovesFocus = false;
      return;
    }
    if (tabMovesFocus) return;
    event.preventDefault();
    const multiline = textarea.value.slice(textarea.selectionStart, textarea.selectionEnd).includes("\n");
    if (event.shiftKey || multiline) {
      indentLines(textarea, event.shiftKey);
    } else {
      insert(textarea, indent);
    }
  });

  document.addEventListener("focusin", () => (tabMovesFocus = false));

  window.addEventListener("beforeunload", (event) => {
    if (unsaved(editor())) {
      event.preventDefault();
      event.returnValue = "";
    }
  });
})();
