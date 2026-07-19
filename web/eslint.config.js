// ESLint flat config for the single-file UI. Intentionally minimal: recommended base +
// react-hooks rules (catch real errors like missing hook deps). No Prettier —
// the dense formatting (one-liner i18n maps) is intentional.
import js from "@eslint/js";
import reactHooks from "eslint-plugin-react-hooks";

export default [
  js.configs.recommended,
  {
    files: ["src/**/*.jsx", "src/**/*.js"],
    plugins: { "react-hooks": reactHooks },
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "module",
      parserOptions: { ecmaFeatures: { jsx: true } },
      globals: {
        window: "readonly",
        document: "readonly",
        navigator: "readonly",
        fetch: "readonly",
        setTimeout: "readonly",
        clearTimeout: "readonly",
        setInterval: "readonly",
        clearInterval: "readonly",
        requestAnimationFrame: "readonly",
        console: "readonly",
        XMLHttpRequest: "readonly",
        // React/MOCK are real imports since the ES-module split (no longer globals).
      },
    },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",
      // `catch (_) {}` pattern + signature placeholders are intentional here.
      // ignoreRestSiblings: `const { status, ...rest } = x` deliberately strips fields.
      "no-unused-vars": ["warn", { args: "none", caughtErrors: "none", varsIgnorePattern: "^_", ignoreRestSiblings: true }],
      "no-empty": ["warn", { allowEmptyCatch: true }],
    },
  },
];
