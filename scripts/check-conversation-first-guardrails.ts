import { spawnSync } from 'node:child_process'
import { readdirSync, readFileSync } from 'node:fs'
import { extname, join, posix, relative, resolve, sep } from 'node:path'
import { ts } from '@ts-morph/common'
import {
  isMap,
  isScalar,
  isSeq,
  LineCounter,
  type Pair,
  parseDocument,
  type YAMLMap,
  type Document as YamlDocument,
  type Node as YamlNode
} from 'yaml'

export interface GuardFinding {
  rule: string
  file: string
  line: number
  message: string
}

export type GuardSources = Readonly<Record<string, string>>

const REAL_REPOSITORY_SOURCES = Symbol('conversation-first-real-repository-sources')
const CHILD_SENTINEL = 'SE_CONVERSATION_FIRST_GUARD_CHILD'
const findingsBySourceIdentity = new WeakMap<GuardSources, GuardFinding[]>()

type ReachabilityScope = 'default' | 'exports'

interface SemanticSymbol {
  module: string
  exported: string
}

interface FunctionNodeInfo {
  key: string
  file: string
  name: string
  node: ts.FunctionLikeDeclaration
  exported: boolean
}

interface WorkflowStep {
  job: string
  index: number
  line: number
  name?: string
  nameScalar: boolean
  run?: string
  runScalar: boolean
  uses?: string
  usesScalar: boolean
  args?: string
  argsPresent: boolean
  argsScalar: boolean
  disabled: boolean
  condition?: string
}

interface ParsedWorkflow {
  file: string
  source: string
  document: YamlDocument.Parsed
  steps: WorkflowStep[]
  data: {
    jobs?: Record<
      string,
      {
        strategy?: { matrix?: { include?: Array<Record<string, unknown>> } }
        steps?: Array<Record<string, unknown>>
      }
    >
  }
}

const PORTABLE_EFFECT_HOOKS = [
  'useTerminalAutoSave',
  'useSessionWorkspaceBootstrap',
  'useConversationHostBootstrap',
  'useConversationLifecycle',
  'useTerminalResourceLifecycle',
  'useTerminalRestore',
  'useCrashRecovery',
  'useTerminalDetachedOutput',
  'useCwd',
  'useGitBranch',
  'useGitStatus',
  'useExitCode',
  'useContextBarSettings',
  'useAppSettingsLoader',
  'useAppliedLanguageSync',
  'useAppliedColorThemeSync',
  'useAppliedUiZoomSync',
  'useKeyboardShortcutsLoader',
  'useProjectsLoader',
  'useProjectsAutoSave',
  'useMenuUpdaterListener',
  'useUpdateCheck',
  'useUpdateToast',
  'useVisibilityState',
  'useTerminalExitNotification',
  'useRemoteProjects',
  'useAcpListeners',
  'useAcpAgents',
  'useAcpHistory',
  'useAcpSessionResume',
  'useAcpMcp',
  'usePreventFileDropNavigation',
  'usePreventNativeContextMenu'
] as const

const PORTABLE_ROUTES = [
  'c/:conversationId',
  'legacy/session/:legacyValue',
  'legacy/storage/:legacyValue',
  'legacy/history/:legacyValue',
  'snapshots',
  'settings',
  'preferences'
] as const

const SHARED_PARSER_ADAPTER_SUFFIXES = [
  'src/renderer/lib/acp-history-persistence.ts',
  'src/renderer/lib/conversation-lifecycle-api.ts',
  'src/renderer/lib/tauri-conversation-api.ts',
  'src/renderer/lib/tauri-session-workspace-api.ts',
  'src/renderer/lib/web-conversation-api.ts',
  'src/renderer/lib/web-session-workspace-api.ts'
] as const

const PR_GUARD_STEP_NAME = 'Check conversation-first guardrails'
const PR_GUARD_RUN = 'bun run check:conversation-first'
const SYNC_STEP_NAME = 'Synchronize stamped root lock entry'
const SYNC_RUN =
  'python3 scripts/sync-stamped-root-lock.py --manifest src-tauri/Cargo.toml --lockfile src-tauri/Cargo.lock --package termul-manager\n' +
  'cargo metadata --locked --manifest-path src-tauri/Cargo.toml --format-version 1 --no-deps'
const WINDOWS_TOKEN_SECURITY_RUN =
  'cargo test --locked web::auth::tests::windows_token_descriptor_rejects_foreign_owner_null_dacl_and_broad_allow_ace -- --exact'

function normalizePath(path: string): string {
  return path.split(sep).join('/')
}

function virtualPath(path: string): string {
  return `/${normalizePath(path).replace(/^\/+/, '')}`
}

function lineAt(sourceFile: ts.SourceFile, node: ts.Node): number {
  return sourceFile.getLineAndCharacterOfPosition(node.getStart(sourceFile)).line + 1
}

function addFinding(
  findings: GuardFinding[],
  rule: string,
  file: string,
  sourceFile: ts.SourceFile,
  node: ts.Node,
  message: string
): void {
  findings.push({ rule, file, line: lineAt(sourceFile, node), message })
}

function scriptKind(file: string): ts.ScriptKind {
  return file.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS
}

function isTypeScriptFile(file: string): boolean {
  return /\.(?:ts|tsx)$/.test(file) && !/\.d\.ts$/.test(file)
}

const SEMANTIC_SOURCE_SUFFIXES = new Set([
  'src/renderer/App.tsx',
  'src/renderer/TauriApp.tsx',
  'src/renderer/app/PortableAppEffects.tsx',
  'src/renderer/app/portable-router.tsx',
  'src/renderer/pages/WorkspaceDashboard.tsx',
  'src/renderer/lib/conversation-api.ts',
  'src/renderer/lib/acp-history-persistence.ts',
  'src/renderer/lib/acp-transport.ts',
  'src/shared/types/session-workspace.types.ts',
  'src/shared/types/web-terminal-protocol.types.ts',
  ...SHARED_PARSER_ADAPTER_SUFFIXES
])

function isSemanticTypeScriptSource(file: string, source: string): boolean {
  const normalized = normalizePath(file)
  return (
    SEMANTIC_SOURCE_SUFFIXES.has(normalized) ||
    (normalized.startsWith('src/renderer/') &&
      /\b(?:terminate|forceKill|force_kill|kill_all|kill|terminateTerminalResource|writeManifest|deleteManifest|saveHistorySession|deleteHistorySession)\b/.test(
        source
      ))
  )
}

function hasModifier(node: ts.Node, kind: ts.SyntaxKind): boolean {
  return (
    ts.canHaveModifiers(node) && Boolean(ts.getModifiers(node)?.some((item) => item.kind === kind))
  )
}

function isFunctionNode(node: ts.Node): node is ts.FunctionLikeDeclaration {
  return (
    ts.isFunctionDeclaration(node) ||
    ts.isFunctionExpression(node) ||
    ts.isArrowFunction(node) ||
    ts.isMethodDeclaration(node)
  )
}

function functionName(node: ts.FunctionLikeDeclaration): string {
  if ('name' in node && node.name && ts.isIdentifier(node.name)) return node.name.text
  if (
    (ts.isFunctionExpression(node) || ts.isArrowFunction(node)) &&
    ts.isVariableDeclaration(node.parent) &&
    ts.isIdentifier(node.parent.name)
  ) {
    return node.parent.name.text
  }
  return '<anonymous>'
}

function functionIsExported(node: ts.FunctionLikeDeclaration): boolean {
  if (ts.isFunctionDeclaration(node)) {
    return (
      hasModifier(node, ts.SyntaxKind.ExportKeyword) ||
      hasModifier(node, ts.SyntaxKind.DefaultKeyword)
    )
  }
  if (
    (ts.isFunctionExpression(node) || ts.isArrowFunction(node)) &&
    ts.isVariableDeclaration(node.parent) &&
    ts.isVariableDeclarationList(node.parent.parent) &&
    ts.isVariableStatement(node.parent.parent.parent)
  ) {
    return hasModifier(node.parent.parent.parent, ts.SyntaxKind.ExportKeyword)
  }
  for (let owner: ts.Node | undefined = node.parent; owner; owner = owner.parent) {
    if (
      ts.isVariableDeclaration(owner) &&
      ts.isVariableDeclarationList(owner.parent) &&
      ts.isVariableStatement(owner.parent.parent) &&
      hasModifier(owner.parent.parent, ts.SyntaxKind.ExportKeyword)
    ) {
      return true
    }
    if (ts.isClassDeclaration(owner)) {
      return (
        hasModifier(owner, ts.SyntaxKind.ExportKeyword) ||
        hasModifier(owner, ts.SyntaxKind.DefaultKeyword)
      )
    }
    if (isFunctionNode(owner)) return functionIsExported(owner)
  }
  return false
}

function constantBoolean(
  expression: ts.Expression,
  seen = new Set<ts.Expression>()
): boolean | undefined {
  if (seen.has(expression)) return undefined
  seen.add(expression)
  if (expression.kind === ts.SyntaxKind.TrueKeyword) return true
  if (expression.kind === ts.SyntaxKind.FalseKeyword) return false
  if (ts.isParenthesizedExpression(expression)) return constantBoolean(expression.expression, seen)
  if (ts.isAsExpression(expression) || ts.isTypeAssertionExpression(expression))
    return constantBoolean(expression.expression, seen)
  if (
    ts.isPrefixUnaryExpression(expression) &&
    expression.operator === ts.SyntaxKind.ExclamationToken
  ) {
    const value = constantBoolean(expression.operand, seen)
    return value === undefined ? undefined : !value
  }
  if (ts.isBinaryExpression(expression)) {
    const left = constantBoolean(expression.left, seen)
    const right = constantBoolean(expression.right, seen)
    if (expression.operatorToken.kind === ts.SyntaxKind.AmpersandAmpersandToken) {
      if (left === false || right === false) return false
      if (left === true && right === true) return true
      return undefined
    }
    if (expression.operatorToken.kind === ts.SyntaxKind.BarBarToken) {
      if (left === true || right === true) return true
      if (left === false && right === false) return false
      return undefined
    }
    if (expression.operatorToken.kind === ts.SyntaxKind.QuestionQuestionToken) {
      return left === undefined ? right : left
    }
  }
  if (ts.isIdentifier(expression)) {
    const initializer = constInitializerForIdentifier(expression)
    if (initializer) return constantBoolean(initializer, seen)
  }
  return undefined
}

function bindingElementExportedName(element: ts.BindingElement): string | undefined {
  if (element.propertyName) {
    if (ts.isIdentifier(element.propertyName) || ts.isStringLiteral(element.propertyName)) {
      return element.propertyName.text
    }
    return undefined
  }
  return ts.isIdentifier(element.name) ? element.name.text : undefined
}

function bindingPatternInitializer(node: ts.Node): ts.Expression | undefined {
  for (let current: ts.Node | undefined = node.parent; current; current = current.parent) {
    if (ts.isVariableDeclaration(current) && current.initializer) return current.initializer
    if (ts.isParameter(current) && current.initializer) return current.initializer
  }
  return undefined
}

function constInitializerForIdentifier(identifier: ts.Identifier): ts.Expression | undefined {
  const sourceFile = identifier.getSourceFile()
  let found: ts.Expression | undefined
  const visit = (node: ts.Node): void => {
    if (
      ts.isVariableDeclaration(node) &&
      node.initializer &&
      ts.isIdentifier(node.name) &&
      node.name.text === identifier.text &&
      node.parent &&
      ts.isVariableDeclarationList(node.parent) &&
      (node.parent.flags & ts.NodeFlags.Const) !== 0
    ) {
      found = node.initializer
    }
    ts.forEachChild(node, visit)
  }
  visit(sourceFile)
  return found
}

function containsNode(container: ts.Node, candidate: ts.Node): boolean {
  return candidate.pos >= container.pos && candidate.end <= container.end
}

function isStaticallyDead(node: ts.Node): boolean {
  let child: ts.Node = node
  for (let parent = node.parent; parent; child = parent, parent = parent.parent) {
    if (ts.isBinaryExpression(parent)) {
      const operator = parent.operatorToken.kind
      const left = constantBoolean(parent.left)
      if (
        operator === ts.SyntaxKind.AmpersandAmpersandToken &&
        left === false &&
        containsNode(parent.right, child)
      ) {
        return true
      }
      if (
        operator === ts.SyntaxKind.BarBarToken &&
        left === true &&
        containsNode(parent.right, child)
      ) {
        return true
      }
    }
    if (ts.isIfStatement(parent)) {
      const value = constantBoolean(parent.expression)
      if (value === false && containsNode(parent.thenStatement, child)) return true
      if (value === true && parent.elseStatement && containsNode(parent.elseStatement, child)) {
        return true
      }
    }
    if (ts.isConditionalExpression(parent)) {
      const value = constantBoolean(parent.condition)
      if (value === false && containsNode(parent.whenTrue, child)) return true
      if (value === true && containsNode(parent.whenFalse, child)) return true
    }
    if (
      (ts.isWhileStatement(parent) || ts.isDoStatement(parent)) &&
      constantBoolean(parent.expression) === false &&
      containsNode(parent.statement, child)
    ) {
      return true
    }
    if (ts.isBlock(parent)) {
      const containingIndex = parent.statements.findIndex((statement) =>
        containsNode(statement, child)
      )
      if (
        containingIndex > 0 &&
        parent.statements
          .slice(0, containingIndex)
          .some((statement) => ts.isReturnStatement(statement) || ts.isThrowStatement(statement))
      ) {
        return true
      }
    }
  }
  return false
}

class TypeScriptProject {
  readonly checker: ts.TypeChecker
  private readonly absoluteSources = new Map<string, string>()
  private readonly sourceFiles = new Map<string, ts.SourceFile>()
  private readonly functions = new Map<string, FunctionNodeInfo>()
  private readonly nodeFunctionKeys = new Map<ts.FunctionLikeDeclaration, string>()
  private readonly functionKeysByFile = new Map<string, Set<string>>()
  private readonly exportedKeysByFile = new Map<string, Set<string>>()
  private readonly defaultKeysByFile = new Map<string, Set<string>>()
  private readonly moduleRootByFile = new Map<string, string>()
  private readonly graph = new Map<string, Set<string>>()
  private readonly callsByFile = new Map<string, ts.CallExpression[]>()
  private readonly jsxByFile = new Map<string, ts.JsxOpeningLikeElement[]>()
  private readonly reachableCache = new Map<string, Set<string>>()

  constructor(readonly sources: GuardSources) {
    for (const [file, source] of Object.entries(sources)) {
      if (isTypeScriptFile(file)) this.absoluteSources.set(virtualPath(file), source)
    }
    const options: ts.CompilerOptions = {
      allowJs: false,
      jsx: ts.JsxEmit.ReactJSX,
      module: ts.ModuleKind.ESNext,
      moduleResolution: ts.ModuleResolutionKind.Bundler,
      noLib: true,
      noResolve: false,
      skipLibCheck: true,
      strict: false,
      target: ts.ScriptTarget.Latest
    }
    const defaultHost = ts.createCompilerHost(options, true)
    const host: ts.CompilerHost = {
      ...defaultHost,
      fileExists: (path) => this.absoluteSources.has(normalizePath(path)),
      readFile: (path) => this.absoluteSources.get(normalizePath(path)),
      getCurrentDirectory: () => '/',
      getCanonicalFileName: (path) => normalizePath(path),
      getNewLine: () => '\n',
      useCaseSensitiveFileNames: () => true,
      writeFile: () => undefined,
      getSourceFile: (path, languageVersion) => {
        const normalized = normalizePath(path)
        const source = this.absoluteSources.get(normalized)
        if (source === undefined) return undefined
        return ts.createSourceFile(
          normalized,
          source,
          languageVersion,
          true,
          scriptKind(normalized)
        )
      },
      resolveModuleNames: (moduleNames, containingFile) =>
        moduleNames.map((moduleName) => {
          const resolved = this.resolveModule(moduleName, containingFile)
          return resolved
            ? {
                resolvedFileName: resolved,
                extension: resolved.endsWith('.tsx') ? ts.Extension.Tsx : ts.Extension.Ts,
                isExternalLibraryImport: false
              }
            : undefined
        })
    }
    const program = ts.createProgram([...this.absoluteSources.keys()], options, host)
    this.checker = program.getTypeChecker()
    for (const absolute of this.absoluteSources.keys()) {
      const sourceFile = program.getSourceFile(absolute)
      if (sourceFile) this.sourceFiles.set(absolute.slice(1), sourceFile)
    }
    this.indexFunctions()
    this.indexEdges()
  }

  private resolveModule(moduleName: string, containingFile: string): string | undefined {
    let base: string
    if (moduleName.startsWith('@/')) {
      base = `/src/renderer/${moduleName.slice(2)}`
    } else if (moduleName.startsWith('@shared/')) {
      base = `/src/shared/${moduleName.slice('@shared/'.length)}`
    } else if (moduleName.startsWith('.')) {
      base = posix.normalize(posix.join(posix.dirname(normalizePath(containingFile)), moduleName))
    } else {
      return undefined
    }
    for (const candidate of [
      base,
      `${base}.ts`,
      `${base}.tsx`,
      `${base}/index.ts`,
      `${base}/index.tsx`
    ]) {
      if (this.absoluteSources.has(candidate)) return candidate
    }
    return undefined
  }

  sourceFile(file: string): ts.SourceFile | undefined {
    return this.sourceFiles.get(normalizePath(file))
  }

  private moduleRoot(file: string): string {
    let key = this.moduleRootByFile.get(file)
    if (!key) {
      key = `${file}::<module>`
      this.moduleRootByFile.set(file, key)
      this.graph.set(key, new Set())
    }
    return key
  }

  private addFunction(node: ts.FunctionLikeDeclaration, file: string): void {
    const key = `${file}:${node.getStart()}`
    const info: FunctionNodeInfo = {
      key,
      file,
      name: functionName(node),
      node,
      exported: functionIsExported(node)
    }
    this.functions.set(key, info)
    this.nodeFunctionKeys.set(node, key)
    const fileKeys = this.functionKeysByFile.get(file) ?? new Set<string>()
    fileKeys.add(key)
    this.functionKeysByFile.set(file, fileKeys)
    if (info.exported) {
      const exported = this.exportedKeysByFile.get(file) ?? new Set<string>()
      exported.add(key)
      this.exportedKeysByFile.set(file, exported)
    }
    if (
      (ts.isFunctionDeclaration(node) && hasModifier(node, ts.SyntaxKind.DefaultKeyword)) ||
      (ts.isFunctionExpression(node) && hasModifier(node, ts.SyntaxKind.DefaultKeyword))
    ) {
      const defaults = this.defaultKeysByFile.get(file) ?? new Set<string>()
      defaults.add(key)
      this.defaultKeysByFile.set(file, defaults)
    }
    this.graph.set(key, new Set())
  }

  private indexFunctions(): void {
    for (const [file, sourceFile] of this.sourceFiles) {
      this.moduleRoot(file)
      const visit = (node: ts.Node): void => {
        if (isFunctionNode(node) && node.body) this.addFunction(node, file)
        ts.forEachChild(node, visit)
      }
      visit(sourceFile)
    }

    for (const [file, sourceFile] of this.sourceFiles) {
      for (const statement of sourceFile.statements) {
        if (!ts.isExportAssignment(statement) || statement.isExportEquals) continue
        const target = this.targetFunctionKey(statement.expression)
        if (!target) continue
        const defaults = this.defaultKeysByFile.get(file) ?? new Set<string>()
        defaults.add(target)
        this.defaultKeysByFile.set(file, defaults)
      }
    }
  }

  private nearestFunctionKey(node: ts.Node): string | undefined {
    for (let owner = node.parent; owner; owner = owner.parent) {
      if (isFunctionNode(owner)) return this.nodeFunctionKeys.get(owner)
    }
    return undefined
  }

  private declarationsForSymbol(
    symbol: ts.Symbol | undefined,
    seen = new Set<ts.Symbol>()
  ): ts.Declaration[] {
    if (!symbol || seen.has(symbol)) return []
    seen.add(symbol)
    const declarations = [...(symbol.declarations ?? [])]
    if ((symbol.flags & ts.SymbolFlags.Alias) !== 0) {
      const aliased = this.checker.getAliasedSymbol(symbol)
      if (aliased && aliased !== symbol)
        declarations.push(...this.declarationsForSymbol(aliased, seen))
    }
    return declarations
  }

  private targetFunctionKey(expression: ts.Expression): string | undefined {
    let targetExpression = expression
    while (ts.isParenthesizedExpression(targetExpression))
      targetExpression = targetExpression.expression
    const symbol = this.checker.getSymbolAtLocation(targetExpression)
    for (const declaration of this.declarationsForSymbol(symbol)) {
      if (isFunctionNode(declaration)) {
        const key = this.nodeFunctionKeys.get(declaration)
        if (key) return key
      }
      if (ts.isBindingElement(declaration)) {
        const initializer = bindingPatternInitializer(declaration)
        if (initializer && ts.isExpression(initializer)) {
          const key = this.targetFunctionKey(initializer)
          if (key) return key
        }
      }
      if (ts.isVariableDeclaration(declaration) && declaration.initializer) {
        if (isFunctionNode(declaration.initializer)) {
          const key = this.nodeFunctionKeys.get(declaration.initializer)
          if (key) return key
        }
        if (ts.isExpression(declaration.initializer)) {
          const key = this.targetFunctionKey(declaration.initializer)
          if (key) return key
        }
      }
    }
    return undefined
  }

  private addEdge(owner: string, target: string | undefined): void {
    if (target) this.graph.get(owner)?.add(target)
  }

  private indexEdges(): void {
    for (const [file, sourceFile] of this.sourceFiles) {
      const calls: ts.CallExpression[] = []
      const jsx: ts.JsxOpeningLikeElement[] = []
      const visit = (node: ts.Node): void => {
        if (ts.isCallExpression(node)) {
          calls.push(node)
          if (!isStaticallyDead(node)) {
            const owner = this.nearestFunctionKey(node) ?? this.moduleRoot(file)
            this.addEdge(owner, this.targetFunctionKey(node.expression))
            for (const argument of node.arguments) {
              if (isFunctionNode(argument)) this.addEdge(owner, this.nodeFunctionKeys.get(argument))
            }
          }
        }
        if (ts.isJsxOpeningElement(node) || ts.isJsxSelfClosingElement(node)) {
          jsx.push(node)
          if (!isStaticallyDead(node)) {
            const owner = this.nearestFunctionKey(node) ?? this.moduleRoot(file)
            if (ts.isIdentifier(node.tagName) || ts.isPropertyAccessExpression(node.tagName)) {
              this.addEdge(owner, this.targetFunctionKey(node.tagName))
            }
          }
        }
        ts.forEachChild(node, visit)
      }
      visit(sourceFile)
      this.callsByFile.set(file, calls)
      this.jsxByFile.set(file, jsx)
    }
  }

  private seeds(file: string, scope: ReachabilityScope): Set<string> {
    const seeds = new Set<string>([this.moduleRoot(file)])
    const selected =
      scope === 'default' ? this.defaultKeysByFile.get(file) : this.exportedKeysByFile.get(file)
    for (const key of selected ?? []) seeds.add(key)
    return seeds
  }

  private reachable(seeds: Iterable<string>): Set<string> {
    const reached = new Set<string>()
    const pending = [...seeds]
    while (pending.length > 0) {
      const key = pending.pop()
      if (!key || reached.has(key)) continue
      reached.add(key)
      for (const target of this.graph.get(key) ?? []) pending.push(target)
    }
    return reached
  }

  private reachableFor(file: string, scope: ReachabilityScope): Set<string> {
    const cacheKey = `${file}:${scope}`
    let reached = this.reachableCache.get(cacheKey)
    if (!reached) {
      reached = this.reachable(this.seeds(file, scope))
      this.reachableCache.set(cacheKey, reached)
    }
    return reached
  }

  private ownerReachable(node: ts.Node, file: string, scope: ReachabilityScope): boolean {
    if (isStaticallyDead(node)) return false
    const owner = this.nearestFunctionKey(node) ?? this.moduleRoot(file)
    return this.reachableFor(file, scope).has(owner)
  }

  calls(file: string, scope: ReachabilityScope): ts.CallExpression[] {
    return (this.callsByFile.get(file) ?? []).filter((node) =>
      this.ownerReachable(node, file, scope)
    )
  }

  jsx(file: string, scope: ReachabilityScope): ts.JsxOpeningLikeElement[] {
    return (this.jsxByFile.get(file) ?? []).filter((node) => this.ownerReachable(node, file, scope))
  }

  functionNameForNode(node: ts.Node): string {
    const key = this.nearestFunctionKey(node)
    return key ? (this.functions.get(key)?.name ?? '') : ''
  }

  callReachableFromNamedFunction(file: string, call: ts.CallExpression, pattern: RegExp): boolean {
    const owner = this.nearestFunctionKey(call)
    if (!owner || isStaticallyDead(call)) return false
    const seeds = [...(this.functionKeysByFile.get(file) ?? [])].filter((key) =>
      pattern.test(this.functions.get(key)?.name ?? '')
    )
    return this.reachable(seeds).has(owner)
  }
}

class TypeScriptModel {
  readonly sourceFile: ts.SourceFile

  constructor(
    private readonly project: TypeScriptProject,
    readonly file: string,
    readonly source: string
  ) {
    const sourceFile = project.sourceFile(file)
    if (!sourceFile) throw new Error(`missing TypeScript source model for ${file}`)
    this.sourceFile = sourceFile
  }

  private importSemantic(declaration: ts.Declaration): SemanticSymbol | null {
    if (ts.isImportSpecifier(declaration)) {
      const importDeclaration = declaration.parent.parent.parent
      if (
        !ts.isImportDeclaration(importDeclaration) ||
        !ts.isStringLiteral(importDeclaration.moduleSpecifier)
      ) {
        return null
      }
      return {
        module: importDeclaration.moduleSpecifier.text,
        exported: declaration.propertyName?.text ?? declaration.name.text
      }
    }
    if (ts.isImportClause(declaration) && declaration.name) {
      const importDeclaration = declaration.parent
      if (
        ts.isImportDeclaration(importDeclaration) &&
        ts.isStringLiteral(importDeclaration.moduleSpecifier)
      ) {
        return { module: importDeclaration.moduleSpecifier.text, exported: 'default' }
      }
    }
    if (ts.isNamespaceImport(declaration)) {
      const importDeclaration = declaration.parent.parent
      if (
        ts.isImportDeclaration(importDeclaration) &&
        ts.isStringLiteral(importDeclaration.moduleSpecifier)
      ) {
        return { module: importDeclaration.moduleSpecifier.text, exported: '*' }
      }
    }
    return null
  }

  private semanticFromSymbol(
    symbol: ts.Symbol | undefined,
    seen: Set<ts.Symbol>
  ): SemanticSymbol | null {
    if (!symbol || seen.has(symbol)) return null
    seen.add(symbol)
    for (const declaration of symbol.declarations ?? []) {
      const imported = this.importSemantic(declaration)
      if (imported) return imported
      if (ts.isBindingElement(declaration)) {
        const exported = bindingElementExportedName(declaration)
        const initializer = bindingPatternInitializer(declaration)
        if (exported && initializer) {
          const base = this.resolveExpression(initializer, seen)
          return { module: base?.module ?? this.file, exported }
        }
      }
      if (ts.isVariableDeclaration(declaration) && declaration.initializer) {
        const resolved = this.resolveExpression(declaration.initializer, seen)
        if (resolved) return resolved
      }
      if (ts.isFunctionDeclaration(declaration) || ts.isMethodDeclaration(declaration)) {
        return {
          module: normalizePath(declaration.getSourceFile().fileName).replace(/^\//, ''),
          exported: declaration.name?.getText() ?? symbol.name
        }
      }
    }
    if ((symbol.flags & ts.SymbolFlags.Alias) !== 0) {
      const aliased = this.project.checker.getAliasedSymbol(symbol)
      if (aliased && aliased !== symbol) return this.semanticFromSymbol(aliased, seen)
    }
    return { module: this.file, exported: symbol.name }
  }

  resolveExpression(expression: ts.Expression, seen = new Set<ts.Symbol>()): SemanticSymbol | null {
    if (ts.isParenthesizedExpression(expression))
      return this.resolveExpression(expression.expression, seen)
    if (ts.isPropertyAccessExpression(expression)) {
      const base = this.resolveExpression(expression.expression, seen)
      return { module: base?.module ?? this.file, exported: expression.name.text }
    }
    if (
      ts.isElementAccessExpression(expression) &&
      expression.argumentExpression &&
      ts.isStringLiteral(expression.argumentExpression)
    ) {
      const base = this.resolveExpression(expression.expression, seen)
      return { module: base?.module ?? this.file, exported: expression.argumentExpression.text }
    }
    const symbol = this.project.checker.getSymbolAtLocation(expression)
    return this.semanticFromSymbol(symbol, seen)
  }

  calls(scope: ReachabilityScope = 'exports'): ts.CallExpression[] {
    return this.project.calls(this.file, scope)
  }

  jsxTags(
    scope: ReachabilityScope = 'exports'
  ): Array<{ node: ts.JsxOpeningLikeElement; symbol: SemanticSymbol | null }> {
    return this.project.jsx(this.file, scope).map((node) => ({
      node,
      symbol:
        ts.isIdentifier(node.tagName) || ts.isPropertyAccessExpression(node.tagName)
          ? this.resolveExpression(node.tagName)
          : null
    }))
  }

  hasImport(module: string, exported: string): boolean {
    for (const statement of this.sourceFile.statements) {
      if (!ts.isImportDeclaration(statement) || !ts.isStringLiteral(statement.moduleSpecifier))
        continue
      if (statement.moduleSpecifier.text !== module) continue
      const clause = statement.importClause
      if (exported === 'default' && clause?.name) return true
      if (clause?.namedBindings && ts.isNamedImports(clause.namedBindings)) {
        if (
          clause.namedBindings.elements.some(
            (element) => (element.propertyName?.text ?? element.name.text) === exported
          )
        ) {
          return true
        }
      }
    }
    return false
  }

  hasCall(exported: string, module?: string, scope: ReachabilityScope = 'exports'): boolean {
    return this.calls(scope).some((call) => {
      const symbol = this.resolveExpression(call.expression)
      return symbol?.exported === exported && (module === undefined || symbol.module === module)
    })
  }

  hasJsx(exported: string, module?: string, scope: ReachabilityScope = 'exports'): boolean {
    return this.jsxTags(scope).some(
      ({ symbol }) =>
        symbol?.exported === exported && (module === undefined || symbol.module === module)
    )
  }

  countJsx(exported: string, module?: string, scope: ReachabilityScope = 'exports'): number {
    return this.jsxTags(scope).filter(
      ({ symbol }) =>
        symbol?.exported === exported && (module === undefined || symbol.module === module)
    ).length
  }

  ownerFunctionName(node: ts.Node): string {
    return this.project.functionNameForNode(node)
  }

  callReachableFrom(node: ts.CallExpression, pattern: RegExp): boolean {
    return this.project.callReachableFromNamedFunction(this.file, node, pattern)
  }
}

function modelsForSources(sources: GuardSources): TypeScriptModel[] {
  const semanticSources = Object.fromEntries(
    Object.entries(sources).filter(
      ([file, source]) => isTypeScriptFile(file) && isSemanticTypeScriptSource(file, source)
    )
  )
  const project = new TypeScriptProject(semanticSources)
  return Object.entries(semanticSources).map(
    ([file, source]) => new TypeScriptModel(project, normalizePath(file), source)
  )
}

function findModel(models: TypeScriptModel[], suffix: string): TypeScriptModel | undefined {
  return models.find((model) => normalizePath(model.file).endsWith(suffix))
}

function checkRootParity(findings: GuardFinding[], models: TypeScriptModel[]): void {
  const roots = ['src/renderer/App.tsx', 'src/renderer/TauriApp.tsx'] as const
  for (const suffix of roots) {
    const model = findModel(models, suffix)
    if (!model) {
      findings.push({
        rule: 'source-discovery',
        file: suffix,
        line: 1,
        message: 'renderer root is missing'
      })
      continue
    }
    const requirements: Array<[boolean, string]> = [
      [
        model.hasImport('@/app/PortableAppEffects', 'PortableAppEffects'),
        'renderer root must import PortableAppEffects from the shared portable effects module'
      ],
      [
        model.hasImport('@/app/portable-router', 'createPortableRouter'),
        'renderer root must import createPortableRouter from the shared portable router module'
      ],
      [
        model.hasCall('createPortableRouter', '@/app/portable-router', 'default'),
        'renderer root must execute the imported createPortableRouter (aliases are supported)'
      ],
      [
        model.hasJsx('PortableAppEffects', '@/app/PortableAppEffects', 'default'),
        'renderer root must render the imported PortableAppEffects component'
      ],
      [
        model.hasJsx(
          'ConversationHostStatus',
          '@/components/conversation/ConversationHostStatus',
          'default'
        ),
        'renderer root must render the imported ConversationHostStatus component'
      ],
      [
        model.countJsx(
          'ConversationRecoveryPanel',
          '@/components/conversation/ConversationRecoveryPanel',
          'default'
        ) === 1,
        'renderer root must execute exactly one imported ConversationRecoveryPanel owner'
      ]
    ]
    for (const [passed, message] of requirements) {
      if (!passed) findings.push({ rule: 'root-parity', file: model.file, line: 1, message })
    }
    for (const call of model.calls('default')) {
      const symbol = model.resolveExpression(call.expression)
      if (symbol?.exported === 'createHashRouter') {
        addFinding(
          findings,
          'root-parity',
          model.file,
          model.sourceFile,
          call,
          'renderer root must not redeclare the portable route table'
        )
      }
      if (
        symbol &&
        PORTABLE_EFFECT_HOOKS.includes(symbol.exported as (typeof PORTABLE_EFFECT_HOOKS)[number])
      ) {
        addFinding(
          findings,
          'root-parity',
          model.file,
          model.sourceFile,
          call,
          `renderer root must not duplicate portable effect hook ${symbol.exported}`
        )
      }
    }
  }

  const dashboard = findModel(models, 'src/renderer/pages/WorkspaceDashboard.tsx')
  if (
    dashboard &&
    (dashboard.hasImport(
      '@/components/conversation/ConversationRecoveryPanel',
      'ConversationRecoveryPanel'
    ) ||
      dashboard.countJsx(
        'ConversationRecoveryPanel',
        '@/components/conversation/ConversationRecoveryPanel'
      ) > 0)
  ) {
    findings.push({
      rule: 'recovery-owner',
      file: dashboard.file,
      line: 1,
      message:
        'WorkspaceDashboard must inherit the renderer-root recovery owner, not mount another panel'
    })
  }

  const effects = findModel(models, 'src/renderer/app/PortableAppEffects.tsx')
  if (!effects) {
    findings.push({
      rule: 'source-discovery',
      file: 'src/renderer/app/PortableAppEffects.tsx',
      line: 1,
      message: 'shared portable effects source is missing'
    })
  } else {
    for (const hook of PORTABLE_EFFECT_HOOKS) {
      if (!effects.hasCall(hook)) {
        findings.push({
          rule: 'root-parity',
          file: effects.file,
          line: 1,
          message: `shared portable effects are missing reachable hook call ${hook}`
        })
      }
    }
    if (!effects.hasCall('initNotificationPermissions')) {
      findings.push({
        rule: 'root-parity',
        file: effects.file,
        line: 1,
        message: 'shared portable effects are missing reachable initNotificationPermissions call'
      })
    }
  }

  const router = findModel(models, 'src/renderer/app/portable-router.tsx')
  if (!router) {
    findings.push({
      rule: 'source-discovery',
      file: 'src/renderer/app/portable-router.tsx',
      line: 1,
      message: 'shared portable router source is missing'
    })
  } else {
    const routes = new Set<string>()
    const visit = (node: ts.Node): void => {
      if (
        ts.isPropertyAssignment(node) &&
        (ts.isIdentifier(node.name) || ts.isStringLiteral(node.name)) &&
        node.name.text === 'path' &&
        ts.isStringLiteralLike(node.initializer) &&
        !isStaticallyDead(node)
      ) {
        routes.add(node.initializer.text)
      }
      ts.forEachChild(node, visit)
    }
    visit(router.sourceFile)
    for (const route of PORTABLE_ROUTES) {
      if (!routes.has(route)) {
        findings.push({
          rule: 'root-parity',
          file: router.file,
          line: 1,
          message: `shared portable router is missing structural path ${route}`
        })
      }
    }
  }
}

function checkNavigationAndLegacy(findings: GuardFinding[], models: TypeScriptModel[]): void {
  const navigationName =
    /(?:navigate|selectProject|switchProject|closeTerminalView|PortableAppEffects|App|TauriApp)/i
  const forbiddenTeardown = new Set([
    'terminate',
    'kill',
    'forceKill',
    'kill_all',
    'terminateTerminalResource'
  ])
  const legacyWrites = new Set([
    'writeManifest',
    'deleteManifest',
    'saveHistorySession',
    'deleteHistorySession'
  ])

  for (const model of models.filter((item) => item.file.startsWith('src/renderer/'))) {
    for (const call of model.calls()) {
      const symbol = model.resolveExpression(call.expression)
      if (!symbol) continue
      if (
        legacyWrites.has(symbol.exported) &&
        /(?:conversation-store|session-workspace-sync)/.test(model.file)
      ) {
        addFinding(
          findings,
          'legacy-read-only',
          model.file,
          model.sourceFile,
          call,
          'Conversation-first renderer paths must not mutate legacy stores'
        )
      }
      if (!forbiddenTeardown.has(symbol.exported)) continue
      const ownerName = model.ownerFunctionName(call)
      const rootFile = /(?:^|\/)(?:App|TauriApp)\.tsx$/.test(model.file)
      if (
        rootFile ||
        navigationName.test(ownerName) ||
        model.callReachableFrom(call, navigationName)
      ) {
        addFinding(
          findings,
          'navigation-preserves-pty',
          model.file,
          model.sourceFile,
          call,
          `navigation helper ${ownerName || '<module>'} must not call ${symbol.exported}`
        )
      }
    }
  }
}

function interfaceMembers(model: TypeScriptModel, name: string): Set<string> {
  const members = new Set<string>()
  for (const statement of model.sourceFile.statements) {
    if (!ts.isInterfaceDeclaration(statement) || statement.name.text !== name) continue
    for (const member of statement.members) {
      if ('name' in member && member.name) {
        if (ts.isIdentifier(member.name) || ts.isStringLiteral(member.name))
          members.add(member.name.text)
      }
    }
  }
  return members
}

function checkTypeContracts(findings: GuardFinding[], models: TypeScriptModel[]): void {
  const workspace = findModel(models, 'src/shared/types/session-workspace.types.ts')
  if (workspace) {
    const members = interfaceMembers(workspace, 'SessionWorkspaceV1')
    for (const field of ['projectId', 'sessionId']) {
      if (members.has(field)) {
        findings.push({
          rule: 'workspace-identity',
          file: workspace.file,
          line: 1,
          message: `SessionWorkspaceV1 must not persist ${field}`
        })
      }
    }
    for (const field of ['claim', 'rawClaim', 'env', 'envVars', 'credentials', 'terminalOutput']) {
      if (members.has(field)) {
        findings.push({
          rule: 'raw-claim',
          file: workspace.file,
          line: 1,
          message: `SessionWorkspaceV1 must not persist secret-bearing field ${field}`
        })
      }
    }
  }

  const terminal = findModel(models, 'src/shared/types/web-terminal-protocol.types.ts')
  if (terminal) {
    const members = interfaceMembers(terminal, 'TerminalSpawnIntentV1')
    for (const field of ['program', 'args', 'env', 'cwd', 'shell']) {
      if (members.has(field)) {
        findings.push({
          rule: 'remote-terminal-intent',
          file: terminal.file,
          line: 1,
          message: `remote TerminalSpawnIntentV1 must not expose caller-controlled ${field}`
        })
      }
    }
  }
}

function checkFacades(findings: GuardFinding[], models: TypeScriptModel[]): void {
  for (const suffix of SHARED_PARSER_ADAPTER_SUFFIXES) {
    const model = findModel(models, suffix)
    if (!model) continue
    const parserImported =
      model.hasImport('@shared/types/conversation.types', 'isConversationId') ||
      model.hasImport('@shared/types/conversation.types', 'parseConversationId')
    const parserCalled = model.hasCall('isConversationId') || model.hasCall('parseConversationId')
    if (!parserImported || !parserCalled) {
      findings.push({
        rule: 'shared-conversation-id-parser',
        file: model.file,
        line: 1,
        message:
          'renderer Conversation adapter must reach the imported shared ConversationId parser'
      })
    }
  }

  const facade = findModel(models, 'src/renderer/lib/conversation-api.ts')
  if (facade) {
    for (const forbidden of ['invoke', 'fetch', 'WebSocket']) {
      for (const call of facade.calls()) {
        if (facade.resolveExpression(call.expression)?.exported === forbidden) {
          addFinding(
            findings,
            'facade-transport-ownership',
            facade.file,
            facade.sourceFile,
            call,
            `compatibility Conversation facade must not own ${forbidden} transport logic`
          )
        }
      }
    }
    for (const delegate of [
      'sessionWorkspaceApi',
      'conversationLifecycleApi',
      'tauriConversationApi',
      'webConversationApi'
    ]) {
      if (
        ![...facade.sourceFile.statements].some((statement) => {
          if (!ts.isImportDeclaration(statement) || !statement.importClause?.namedBindings)
            return false
          if (!ts.isNamedImports(statement.importClause.namedBindings)) return false
          return statement.importClause.namedBindings.elements.some(
            (element) => (element.propertyName?.text ?? element.name.text) === delegate
          )
        })
      ) {
        findings.push({
          rule: 'facade-transport-ownership',
          file: facade.file,
          line: 1,
          message: `compatibility facade must import production delegate ${delegate}`
        })
      }
    }
  }

  const history = findModel(models, 'src/renderer/lib/acp-history-persistence.ts')
  if (history) {
    if (
      !history.hasImport('@/lib/acp-history-api', 'acpHistoryApi') ||
      !history.hasCall('getPage', '@/lib/acp-history-api')
    ) {
      findings.push({
        rule: 'history-paging-facade',
        file: history.file,
        line: 1,
        message: 'desktop history paging must reach acpHistoryApi.getPage through the real facade'
      })
    }
    if (!history.hasCall('getSessionPayloadPage')) {
      findings.push({
        rule: 'history-paging-facade',
        file: history.file,
        line: 1,
        message: 'server history paging must reach transport.getSessionPayloadPage'
      })
    }
  }
}

function checkRendererCredential(findings: GuardFinding[], models: TypeScriptModel[]): void {
  const transport = findModel(models, 'src/renderer/lib/acp-transport.ts')
  if (!transport) return
  if (!transport.hasCall('getRemoteAccessCredential')) {
    findings.push({
      rule: 'authenticated-remote-access',
      file: transport.file,
      line: 1,
      message: 'renderer WebSocket transport must reach the in-memory credential boundary'
    })
  }
  const visit = (node: ts.Node): void => {
    if (ts.isPropertyAssignment(node)) {
      const name = ts.isIdentifier(node.name) || ts.isStringLiteral(node.name) ? node.name.text : ''
      if (
        (name === 'token' || name === 'credential') &&
        ts.isStringLiteralLike(node.initializer) &&
        ['dev', 'placeholder', 'changeme'].includes(node.initializer.text.toLowerCase()) &&
        !isStaticallyDead(node)
      ) {
        addFinding(
          findings,
          'authenticated-remote-access',
          transport.file,
          transport.sourceFile,
          node,
          'placeholder remote authentication credential is forbidden'
        )
      }
    }
    ts.forEachChild(node, visit)
  }
  visit(transport.sourceFile)
}

function mapPair(map: YAMLMap, key: string): Pair | undefined {
  return map.items.find(
    (pair): pair is Pair => isScalar(pair.key) && String(pair.key.value) === key
  )
}

function scalarText(node: YamlNode | null | undefined): string | undefined {
  return isScalar(node) && typeof node.value === 'string' ? node.value : undefined
}

function yamlLine(node: YamlNode | null | undefined, counter: LineCounter): number {
  return node?.range ? counter.linePos(node.range[0]).line : 1
}

function constantWorkflowCondition(value: unknown): boolean | undefined {
  if (value === false) return false
  if (value === true) return true
  if (typeof value !== 'string') return undefined
  const trimmed = value.trim()
  if (trimmed === 'false' || trimmed === '${{ false }}' || trimmed === '${{false}}') return false
  if (trimmed === 'true' || trimmed === '${{ true }}' || trimmed === '${{true}}') return true
  return undefined
}

function yamlCondition(node: YamlNode | null | undefined): {
  text?: string
  disabled: boolean
} {
  if (!isScalar(node)) return { disabled: false }
  const text =
    typeof node.value === 'string' || typeof node.value === 'boolean'
      ? String(node.value)
      : undefined
  return { text, disabled: constantWorkflowCondition(node.value) === false }
}

function parseWorkflow(
  file: string,
  source: string,
  findings: GuardFinding[]
): ParsedWorkflow | undefined {
  const counter = new LineCounter()
  const document = parseDocument(source, { lineCounter: counter })
  if (document.errors.length > 0) {
    findings.push({
      rule: 'workflow-yaml',
      file,
      line: 1,
      message: `workflow YAML parse failed: ${document.errors[0]?.message ?? 'unknown parse error'}`
    })
    return undefined
  }
  if (!isMap(document.contents)) {
    findings.push({
      rule: 'workflow-yaml',
      file,
      line: 1,
      message: 'workflow YAML root must be a map'
    })
    return undefined
  }
  const jobsNode = mapPair(document.contents, 'jobs')?.value
  const steps: WorkflowStep[] = []
  if (isMap(jobsNode)) {
    for (const jobPair of jobsNode.items) {
      const job = isScalar(jobPair.key) ? String(jobPair.key.value) : '<job>'
      if (!isMap(jobPair.value)) continue
      const jobCondition = yamlCondition(
        mapPair(jobPair.value, 'if')?.value as YamlNode | null | undefined
      )
      const stepsNode = mapPair(jobPair.value, 'steps')?.value
      if (!isSeq(stepsNode)) continue
      for (const [index, item] of stepsNode.items.entries()) {
        if (!isMap(item)) continue
        const nameNode = mapPair(item, 'name')?.value as YamlNode | null | undefined
        const runNode = mapPair(item, 'run')?.value as YamlNode | null | undefined
        const usesNode = mapPair(item, 'uses')?.value as YamlNode | null | undefined
        const withNode = mapPair(item, 'with')?.value as YamlNode | null | undefined
        const argsNode = (isMap(withNode) ? mapPair(withNode, 'args')?.value : undefined) as
          | YamlNode
          | null
          | undefined
        const stepCondition = yamlCondition(
          mapPair(item, 'if')?.value as YamlNode | null | undefined
        )
        steps.push({
          job,
          index,
          line: yamlLine(item, counter),
          name: scalarText(nameNode),
          nameScalar: isScalar(nameNode) && typeof nameNode.value === 'string',
          run: scalarText(runNode),
          runScalar: isScalar(runNode) && typeof runNode.value === 'string',
          uses: scalarText(usesNode),
          usesScalar: isScalar(usesNode) && typeof usesNode.value === 'string',
          args: scalarText(argsNode),
          argsPresent: argsNode !== undefined,
          argsScalar: isScalar(argsNode) && typeof argsNode.value === 'string',
          disabled: jobCondition.disabled || stepCondition.disabled,
          condition: stepCondition.text ?? jobCondition.text
        })
      }
    }
  }
  return {
    file,
    source,
    document,
    steps,
    data: document.toJS() as ParsedWorkflow['data']
  }
}

function shellFragments(command: string): string[] {
  return command
    .replace(/\\\r?\n/g, ' ')
    .split(/\r?\n|&&|\|\||;|(?<!\|)\|(?!\|)/)
    .map((fragment) => fragment.trim())
    .filter(Boolean)
}

function cargoOccurrences(command: string): Array<{ kind: string; invocation: string }> {
  const occurrences: Array<{ kind: string; invocation: string }> = []
  for (const fragment of shellFragments(command)) {
    const matches = [...fragment.matchAll(/\bcargo\s+(metadata|check|test|clippy|build)\b/g)]
    for (const [index, match] of matches.entries()) {
      const start = match.index ?? 0
      const end = matches[index + 1]?.index ?? fragment.length
      occurrences.push({ kind: match[1], invocation: fragment.slice(start, end).trim() })
    }
  }
  return occurrences
}

function invocationIsLocked(invocation: string): boolean {
  return /(?:^|[\s"'])--locked(?=$|[\s"'])/.test(invocation)
}

function stepDisplay(step: WorkflowStep): string {
  return step.name ?? String(step.index)
}

function checkCargoRuns(findings: GuardFinding[], workflows: ParsedWorkflow[]): void {
  for (const workflow of workflows) {
    for (const step of workflow.steps) {
      if (step.disabled || !step.runScalar || step.run === undefined) continue
      for (const occurrence of cargoOccurrences(step.run)) {
        if (invocationIsLocked(occurrence.invocation)) continue
        findings.push({
          rule: 'locked-rust-ci',
          file: workflow.file,
          line: step.line,
          message: `job=${step.job} step=${stepDisplay(step)} cargo ${occurrence.kind} command must use --locked`
        })
      }
    }
  }
}

function checkDefaultPrGuard(findings: GuardFinding[], validation: ParsedWorkflow): void {
  const named = validation.steps.filter(
    (step) => !step.disabled && step.nameScalar && step.name === PR_GUARD_STEP_NAME
  )
  if (named.length !== 1) {
    findings.push({
      rule: 'default-pr-guard',
      file: validation.file,
      line: named[0]?.line ?? 1,
      message: `workflow must contain exactly one step named ${JSON.stringify(PR_GUARD_STEP_NAME)}; found ${named.length}`
    })
    return
  }
  if (!named[0].runScalar || named[0].run !== PR_GUARD_RUN) {
    findings.push({
      rule: 'default-pr-guard',
      file: validation.file,
      line: named[0].line,
      message: `step ${JSON.stringify(PR_GUARD_STEP_NAME)} run scalar must equal ${JSON.stringify(PR_GUARD_RUN)}`
    })
  }
}

function checkNativeCi(findings: GuardFinding[], validation: ParsedWorkflow): void {
  const jobs = validation.data.jobs ?? {}
  const durability = jobs['conversation-native-durability']
  const include = durability?.strategy?.matrix?.include
  const platforms = new Set(
    Array.isArray(include)
      ? include
          .map((item) => item.platform)
          .filter((value): value is string => typeof value === 'string')
      : []
  )
  for (const platform of ['linux', 'macos', 'windows']) {
    if (!platforms.has(platform)) {
      findings.push({
        rule: 'native-ci-wiring',
        file: validation.file,
        line: 1,
        message: `locked native durability matrix is missing ${platform}`
      })
    }
  }

  const exactRuns = new Set(
    validation.steps.flatMap((step) =>
      !step.disabled && step.runScalar && step.run !== undefined
        ? step.run
            .split(/\r?\n/)
            .map((line) => line.trim())
            .filter(Boolean)
        : []
    )
  )
  for (const required of [
    'cargo test --locked conversation::native_durability_tests',
    'cargo test --locked --test conversation_first_guardrails',
    'cargo build --locked --bin termul-server --features standalone-server',
    'cargo clippy --locked --bin termul-server --features standalone-server -- -D warnings',
    WINDOWS_TOKEN_SECURITY_RUN
  ]) {
    if (!exactRuns.has(required)) {
      findings.push({
        rule: 'native-ci-wiring',
        file: validation.file,
        line: 1,
        message: `locked native/semantic CI wiring is missing exact run ${required}`
      })
    }
  }

  const windowsSteps = validation.steps.filter(
    (step) => !step.disabled && step.job === 'rust-windows-check'
  )
  if (!windowsSteps.some((step) => step.runScalar && step.run === WINDOWS_TOKEN_SECURITY_RUN)) {
    findings.push({
      rule: 'native-ci-wiring',
      file: validation.file,
      line: 1,
      message:
        'rust-windows-check must execute the cfg(windows) token owner/DACL/reparse security test'
    })
  }
}

function checkStampedPackaging(findings: GuardFinding[], workflow: ParsedWorkflow): void {
  const isNightly = workflow.file.endsWith('/nightly.yml')
  const isRelease = workflow.file.endsWith('/release.yml')
  if (!isNightly && !isRelease) return
  const stampName = isNightly ? 'Stamp nightly version into sources' : 'Align app versions with tag'
  for (const job of ['build', 'standalone-server']) {
    const steps = workflow.steps.filter((step) => step.job === job)
    const stamps = steps.filter((step) => step.nameScalar && step.name === stampName)
    if (stamps.length !== 1) {
      findings.push({
        rule: 'stamped-root-lock',
        file: workflow.file,
        line: stamps[0]?.line ?? 1,
        message: `job=${job} must contain exactly one stamp step ${JSON.stringify(stampName)}; found ${stamps.length}`
      })
      continue
    }
    const stampPosition = steps.indexOf(stamps[0])
    const next = steps[stampPosition + 1]
    if (!next || !next.nameScalar || next.name !== SYNC_STEP_NAME) {
      findings.push({
        rule: 'stamped-root-lock',
        file: workflow.file,
        line: stamps[0].line,
        message: `job=${job} immediate step after ${JSON.stringify(stampName)} must be ${JSON.stringify(SYNC_STEP_NAME)}`
      })
      continue
    }
    if (!next.runScalar || next.run !== SYNC_RUN) {
      findings.push({
        rule: 'stamped-root-lock',
        file: workflow.file,
        line: next.line,
        message: `job=${job} step=${SYNC_STEP_NAME} must use the exact two-line repository-root synchronization scalar`
      })
    }
  }

  const tauri = workflow.steps.filter(
    (step) => step.usesScalar && step.uses?.startsWith('tauri-apps/tauri-action@')
  )
  if (tauri.length === 0) {
    findings.push({
      rule: 'locked-tauri-action',
      file: workflow.file,
      line: 1,
      message: 'workflow must contain at least one tauri-apps/tauri-action step'
    })
  }
  for (const step of tauri) {
    if (!step.argsPresent || !step.argsScalar || step.args === undefined) {
      findings.push({
        rule: 'locked-tauri-action',
        file: workflow.file,
        line: step.line,
        message: `job=${step.job} step=${stepDisplay(step)} tauri-action with.args must be a scalar`
      })
    } else if (!step.args.endsWith('-- --locked')) {
      findings.push({
        rule: 'locked-tauri-action',
        file: workflow.file,
        line: step.line,
        message: `job=${step.job} step=${stepDisplay(step)} tauri-action with.args must end exactly -- --locked`
      })
    }
  }
}

function checkWorkflows(findings: GuardFinding[], sources: GuardSources): void {
  const discovered = Object.entries(sources)
    .filter(([file]) => /^\.github\/workflows\/.*\.(?:yml|yaml)$/.test(normalizePath(file)))
    .map(([file, source]) => parseWorkflow(normalizePath(file), source, findings))
    .filter((workflow): workflow is ParsedWorkflow => workflow !== undefined)
  if (discovered.length === 0) {
    findings.push({
      rule: 'source-discovery',
      file: '.github/workflows',
      line: 1,
      message: 'no active workflow YAML files were discovered'
    })
    return
  }

  checkCargoRuns(findings, discovered)
  const validation = discovered.find((workflow) => workflow.file.endsWith('/pr-validation.yml'))
  if (!validation) {
    findings.push({
      rule: 'native-ci-wiring',
      file: '.github/workflows/pr-validation.yml',
      line: 1,
      message: 'PR validation workflow is missing'
    })
  } else {
    checkDefaultPrGuard(findings, validation)
    checkNativeCi(findings, validation)
  }
  for (const workflow of discovered) checkStampedPackaging(findings, workflow)
}

/** Compatibility helper retained for callers; semantic checks use compiler nodes. */
export function stripComments(source: string): string {
  const scanner = ts.createScanner(
    ts.ScriptTarget.Latest,
    false,
    ts.LanguageVariant.Standard,
    source
  )
  let result = ''
  let position = 0
  for (let token = scanner.scan(); token !== ts.SyntaxKind.EndOfFileToken; token = scanner.scan()) {
    const start = scanner.getTokenPos()
    const end = scanner.getTextPos()
    result += source.slice(position, start)
    if (
      token === ts.SyntaxKind.SingleLineCommentTrivia ||
      token === ts.SyntaxKind.MultiLineCommentTrivia
    ) {
      result += source.slice(start, end).replace(/[^\n]/g, ' ')
    } else {
      result += source.slice(start, end)
    }
    position = end
  }
  return result + source.slice(position)
}

/** Compatibility helper retained for legacy tests; Rust enforcement lives in the syn guard. */
export function stripRustTestCode(source: string): string {
  return source.replace(/#\[cfg\(test\)\][\s\S]*$/m, (matched) => matched.replace(/[^\n]/g, ' '))
}

function cloneFindings(findings: GuardFinding[]): GuardFinding[] {
  return findings.map((finding) => ({ ...finding }))
}

function isGuardFinding(value: unknown): value is GuardFinding {
  if (!value || typeof value !== 'object') return false
  const finding = value as Partial<GuardFinding>
  return (
    typeof finding.rule === 'string' &&
    typeof finding.file === 'string' &&
    Number.isInteger(finding.line) &&
    (finding.line ?? 0) > 0 &&
    typeof finding.message === 'string'
  )
}

function repositoryRoot(sources: GuardSources): string | undefined {
  return (sources as GuardSources & { [REAL_REPOSITORY_SOURCES]?: string })[REAL_REPOSITORY_SOURCES]
}

function shouldDelegateToBun(sources: GuardSources): boolean {
  return (
    !process.versions.bun &&
    !process.env[CHILD_SENTINEL] &&
    (repositoryRoot(sources) !== undefined || Object.keys(sources).length >= 100)
  )
}

function delegatedFindings(sources: GuardSources): GuardFinding[] {
  const root = repositoryRoot(sources) ?? process.cwd()
  const result = spawnSync('bun', ['scripts/check-conversation-first-guardrails.ts', '--json'], {
    cwd: root,
    encoding: 'utf8',
    env: { ...process.env, [CHILD_SENTINEL]: '1' },
    maxBuffer: 10 * 1024 * 1024
  })
  if (result.error) throw result.error
  let parsed: unknown
  try {
    parsed = JSON.parse(result.stdout)
  } catch (error) {
    throw new Error(
      `conversation-first bun delegate returned invalid JSON: ${error instanceof Error ? error.message : String(error)}`
    )
  }
  if (!Array.isArray(parsed) || !parsed.every(isGuardFinding)) {
    throw new Error('conversation-first bun delegate returned an invalid GuardFinding[] payload')
  }
  const expectedStatus = parsed.length === 0 ? 0 : 1
  if (result.status !== expectedStatus) {
    throw new Error(
      `conversation-first bun delegate exited ${String(result.status)} for ${parsed.length} finding(s): ${result.stderr.trim()}`
    )
  }
  return parsed
}

function inProcessFindings(sources: GuardSources): GuardFinding[] {
  const models = modelsForSources(sources)
  const findings: GuardFinding[] = []
  checkRootParity(findings, models)
  checkNavigationAndLegacy(findings, models)
  checkTypeContracts(findings, models)
  checkFacades(findings, models)
  checkRendererCredential(findings, models)
  checkWorkflows(findings, sources)
  return findings.sort(
    (left, right) =>
      left.file.localeCompare(right.file) ||
      left.line - right.line ||
      left.rule.localeCompare(right.rule) ||
      left.message.localeCompare(right.message)
  )
}

export function checkConversationFirstGuardrails(sources: GuardSources): GuardFinding[] {
  const cached = findingsBySourceIdentity.get(sources)
  if (cached) return cloneFindings(cached)
  const findings = shouldDelegateToBun(sources)
    ? delegatedFindings(sources)
    : inProcessFindings(sources)
  findingsBySourceIdentity.set(sources, cloneFindings(findings))
  return cloneFindings(findings)
}

function walkSources(
  root: string,
  directory: string,
  acceptPath: (path: string) => boolean,
  acceptSource: (file: string, source: string) => boolean
): Array<[string, string]> {
  const absolute = resolve(root, directory)
  const sources: Array<[string, string]> = []
  for (const entry of readdirSync(absolute, { withFileTypes: true })) {
    const path = join(absolute, entry.name)
    if (entry.isDirectory()) {
      sources.push(
        ...walkSources(root, normalizePath(relative(root, path)), acceptPath, acceptSource)
      )
    } else if (entry.isFile() && acceptPath(path)) {
      const file = normalizePath(relative(root, path))
      const source = readFileSync(path, 'utf8')
      if (acceptSource(file, source)) sources.push([file, source])
    }
  }
  return sources
}

export function loadRepositorySources(root = process.cwd()): GuardSources {
  const resolvedRoot = resolve(root)
  const isProductionTypeScript = (path: string): boolean => {
    const normalized = normalizePath(path)
    return (
      ['.ts', '.tsx'].includes(extname(path)) &&
      !path.endsWith('.d.ts') &&
      !normalized.includes('/__tests__/') &&
      !/\.(?:test|spec)\.(?:ts|tsx)$/.test(normalized)
    )
  }
  const sourceEntries = [
    ...walkSources(resolvedRoot, 'src/renderer', isProductionTypeScript, (file, source) =>
      isSemanticTypeScriptSource(file, source)
    ),
    ...walkSources(resolvedRoot, 'src/shared', isProductionTypeScript, (file, source) =>
      isSemanticTypeScriptSource(file, source)
    ),
    ...walkSources(
      resolvedRoot,
      '.github/workflows',
      (path) => ['.yml', '.yaml'].includes(extname(path)),
      () => true
    )
  ].sort(([left], [right]) => left.localeCompare(right))
  const sources = Object.fromEntries(sourceEntries) as GuardSources
  Object.defineProperty(sources, REAL_REPOSITORY_SOURCES, {
    configurable: false,
    enumerable: false,
    value: resolvedRoot,
    writable: false
  })
  return sources
}

export function main(
  sources: GuardSources = loadRepositorySources(),
  options: { json?: boolean } = {}
): number {
  const findings = checkConversationFirstGuardrails(sources)
  if (options.json) {
    console.log(JSON.stringify(findings))
    return findings.length === 0 ? 0 : 1
  }
  if (findings.length === 0) {
    console.log(
      `Conversation-first semantic guardrails passed (${Object.keys(sources).length} discovered sources)`
    )
    return 0
  }
  for (const item of findings) {
    console.error(`${item.file}:${item.line} [${item.rule}] ${item.message}`)
  }
  console.error(`Conversation-first semantic guardrails failed with ${findings.length} finding(s)`)
  return 1
}

if (import.meta.main)
  process.exit(main(loadRepositorySources(), { json: process.argv.includes('--json') }))
