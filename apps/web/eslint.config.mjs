import eslint from '@eslint/js'
import tseslint from 'typescript-eslint'

export default tseslint.config(
  { ignores: ['dist/**', 'node_modules/**', 'test-results/**'] },
  eslint.configs.recommended,
  ...tseslint.configs.strictTypeChecked,
  { languageOptions: { parserOptions: { project: ['./tsconfig.json'], tsconfigRootDir: import.meta.dirname } }, rules: { '@typescript-eslint/consistent-type-imports': 'error', '@typescript-eslint/no-confusing-void-expression': 'off' } }
)
