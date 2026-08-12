import { apiRequest, type BlockedWord, type TerminologyDictionary, type TerminologyEntry } from '../api';
import { refreshIcons } from '../components/icons';
import type { Page } from './page';

type Mode = 'terms' | 'blocked';

export class LexiconPage implements Page {
  private root: HTMLElement | null = null;
  private mode: Mode = 'terms';
  private dictionaries: TerminologyDictionary[] = [];
  private entries: TerminologyEntry[] = [];
  private blocked: BlockedWord[] = [];
  private selectedId = '';

  constructor(private readonly onError: (message: string) => void, private readonly onMessage: (message: string) => void) {}

  async mount(root: HTMLElement) {
    this.root = root;
    root.innerHTML = `
      <main class="lexicon-page app-shell">
        <header class="admin-heading">
          <div><span class="section-kicker"><i data-lucide="library"></i> LANGUAGE POLICY</span><h1>词库管理</h1></div>
          <a class="button-secondary" href="/admin"><i data-lucide="arrow-left"></i><span>返回系统管理</span></a>
        </header>
        <nav class="lexicon-tabs" role="tablist">
          <button class="active" type="button" data-mode="terms"><i data-lucide="book-open-text"></i><span>行业术语</span></button>
          <button type="button" data-mode="blocked"><i data-lucide="shield-ban"></i><span>屏蔽词</span></button>
        </nav>
        <section class="lexicon-workspace" aria-busy="true"></section>
        <dialog class="lexicon-dialog"></dialog>
        <input class="lexicon-file" type="file" accept=".csv,.txt,text/csv,text/plain" hidden>
      </main>`;
    root.querySelector('.lexicon-tabs')?.addEventListener('click', (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-mode]');
      if (!button) return; this.mode = button.dataset.mode as Mode;
      root.querySelectorAll('[data-mode]').forEach((item) => item.classList.toggle('active', item === button));
      this.render();
    });
    root.querySelector('.lexicon-workspace')?.addEventListener('click', (event) => void this.handleAction(event));
    root.querySelector<HTMLInputElement>('.lexicon-file')?.addEventListener('change', (event) => void this.importFile(event));
    refreshIcons(root);
    await this.load();
  }

  destroy() { this.root?.querySelector<HTMLDialogElement>('.lexicon-dialog')?.close(); this.root = null; }

  private async load() {
    const workspace = this.root?.querySelector<HTMLElement>('.lexicon-workspace'); if (workspace) workspace.setAttribute('aria-busy', 'true');
    try {
      [this.dictionaries, this.blocked] = await Promise.all([
        apiRequest<TerminologyDictionary[]>('/api/admin/terminology-dictionaries'),
        apiRequest<BlockedWord[]>('/api/admin/blocked-words'),
      ]);
      if (!this.selectedId && this.dictionaries[0]) this.selectedId = this.dictionaries[0].id;
      await this.loadEntries();
    } catch (error) { this.onError(this.message(error)); }
    finally { if (workspace) workspace.setAttribute('aria-busy', 'false'); this.render(); }
  }

  private async loadEntries() {
    this.entries = this.selectedId ? await apiRequest<TerminologyEntry[]>(`/api/admin/terminology-dictionaries/${this.selectedId}/entries`) : [];
  }

  private render() {
    const workspace = this.root?.querySelector<HTMLElement>('.lexicon-workspace'); if (!workspace) return;
    workspace.innerHTML = this.mode === 'terms' ? this.termView() : this.blockedView(); refreshIcons(workspace);
  }

  private termView() {
    const selected = this.dictionaries.find((item) => item.id === this.selectedId);
    return `<aside class="lexicon-sidebar">
      <div class="lexicon-panel-heading"><div><h2>行业词库</h2><span>${this.dictionaries.length} 个</span></div><button class="icon-button" data-action="new-dictionary" title="新建词库"><i data-lucide="plus"></i></button></div>
      <div class="lexicon-dictionary-list">${this.dictionaries.map((item) => `<button class="${item.id === this.selectedId ? 'active' : ''}" data-action="select-dictionary" data-id="${item.id}"><strong>${this.escape(item.name)}</strong><span>${this.escape(item.industry)} · ${item.status === 'active' ? '启用' : '停用'}</span></button>`).join('') || '<p class="empty-state">尚未创建行业词库</p>'}</div>
    </aside><section class="lexicon-content">${selected ? `
      <div class="lexicon-content-heading"><div><h2>${this.escape(selected.name)}</h2><p>${this.escape(selected.description || `${selected.industry}行业术语`)}</p></div><div class="lexicon-actions"><button class="button-secondary" data-action="import-entries"><i data-lucide="file-up"></i><span>导入 CSV</span></button><button class="button-secondary" data-action="edit-dictionary"><i data-lucide="settings-2"></i><span>词库设置</span></button><button class="icon-button danger" data-action="delete-dictionary" data-id="${selected.id}" title="删除词库"><i data-lucide="trash-2"></i></button><button class="button-primary" data-action="new-entry"><i data-lucide="plus"></i><span>新增术语</span></button></div></div>
      <div class="lexicon-table-wrap"><table><thead><tr><th>标准原词</th><th>别名</th><th>目标译法</th><th>优先级</th><th>状态</th><th><span class="sr-only">操作</span></th></tr></thead><tbody>${this.entries.map((item) => `<tr><td><strong>${this.escape(item.source_term)}</strong></td><td>${this.escape(item.aliases.join('、') || '—')}</td><td>${this.escape(item.target_term)}</td><td>${item.priority}</td><td><span class="status-pill ${item.status}">${item.status === 'active' ? '启用' : '停用'}</span></td><td class="table-actions"><button class="icon-button" data-action="edit-entry" data-id="${item.id}" title="编辑"><i data-lucide="pencil"></i></button><button class="icon-button danger" data-action="delete-entry" data-id="${item.id}" title="删除"><i data-lucide="trash-2"></i></button></td></tr>`).join('') || '<tr><td colspan="6" class="empty-state">暂无术语，可单条新增或导入 CSV</td></tr>'}</tbody></table></div>
      <p class="lexicon-format">CSV 列：source_term,target_term,aliases（用 | 分隔）,priority</p>` : '<div class="empty-state lexicon-empty">创建词库后即可录入行业术语</div>'}</section>`;
  }

  private blockedView() {
    return `<section class="lexicon-content lexicon-content-wide"><div class="lexicon-content-heading"><div><h2>全局屏蔽词</h2><p>对翻译输入、流式译文、保存结果和语音合成统一生效</p></div><div class="lexicon-actions"><button class="button-secondary" data-action="import-blocked"><i data-lucide="file-up"></i><span>导入 CSV / TXT</span></button><button class="button-primary" data-action="new-blocked"><i data-lucide="plus"></i><span>新增屏蔽词</span></button></div></div>
      <div class="lexicon-table-wrap"><table><thead><tr><th>屏蔽词</th><th>替换文本</th><th>匹配</th><th>大小写</th><th>备注</th><th>状态</th><th><span class="sr-only">操作</span></th></tr></thead><tbody>${this.blocked.map((item) => `<tr><td><strong>${this.escape(item.word)}</strong></td><td>${this.escape(item.replacement)}</td><td>${item.match_mode === 'word' ? '完整词' : '包含匹配'}</td><td>${item.case_sensitive ? '区分' : '忽略'}</td><td>${this.escape(item.note || '—')}</td><td><span class="status-pill ${item.status}">${item.status === 'active' ? '启用' : '停用'}</span></td><td class="table-actions"><button class="icon-button" data-action="edit-blocked" data-id="${item.id}" title="编辑"><i data-lucide="pencil"></i></button><button class="icon-button danger" data-action="delete-blocked" data-id="${item.id}" title="删除"><i data-lucide="trash-2"></i></button></td></tr>`).join('') || '<tr><td colspan="7" class="empty-state">暂无屏蔽词</td></tr>'}</tbody></table></div><p class="lexicon-format">CSV 列：word,replacement,match_mode（substring / word）,case_sensitive,note；TXT 可每行一个词</p></section>`;
  }

  private async handleAction(event: Event) {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-action]'); if (!button) return;
    const id = button.dataset.id ?? '';
    if (button.dataset.action === 'select-dictionary') { this.selectedId = id; await this.loadEntries(); this.render(); return; }
    if (button.dataset.action === 'new-dictionary') return this.dictionaryDialog();
    if (button.dataset.action === 'edit-dictionary') return this.dictionaryDialog(this.dictionaries.find((item) => item.id === this.selectedId));
    if (button.dataset.action === 'new-entry') return this.entryDialog();
    if (button.dataset.action === 'edit-entry') return this.entryDialog(this.entries.find((item) => item.id === id));
    if (button.dataset.action === 'new-blocked') return this.blockedDialog();
    if (button.dataset.action === 'edit-blocked') return this.blockedDialog(this.blocked.find((item) => item.id === id));
    if (button.dataset.action === 'import-entries' || button.dataset.action === 'import-blocked') { const file = this.root?.querySelector<HTMLInputElement>('.lexicon-file'); if (file) { file.dataset.kind = button.dataset.action; file.value = ''; file.click(); } return; }
    if (button.dataset.action?.startsWith('delete-') && window.confirm('该记录将软删除并保留历史，确认继续？')) await this.remove(button.dataset.action, id);
  }

  private dictionaryDialog(value?: TerminologyDictionary) { this.openDialog(value ? '编辑行业词库' : '新建行业词库', `<label><span>词库名称</span><input name="name" maxlength="120" required value="${this.attr(value?.name ?? '')}"></label><label><span>行业</span><input name="industry" maxlength="80" required value="${this.attr(value?.industry ?? '')}" placeholder="医疗、法律、制造…"></label><label><span>说明</span><textarea name="description" rows="3">${this.escape(value?.description ?? '')}</textarea></label><div class="editor-language-pair"><label><span>源语言</span><input name="source_language" value="${this.attr(value?.source_language ?? 'auto')}"></label><label><span>目标语言</span><input name="target_language" value="${this.attr(value?.target_language ?? 'zh')}"></label></div>${this.statusField(value?.status)}`, async (data) => { const body = Object.fromEntries(data); const saved = await apiRequest<TerminologyDictionary>(value ? `/api/admin/terminology-dictionaries/${value.id}` : '/api/admin/terminology-dictionaries', { method: value ? 'PATCH' : 'POST', body: JSON.stringify(body) }); this.selectedId = saved.id; await this.load(); }); }

  private entryDialog(value?: TerminologyEntry) { this.openDialog(value ? '编辑术语' : '新增术语', `<label><span>标准原词</span><input name="source_term" required value="${this.attr(value?.source_term ?? '')}"></label><label><span>目标译法</span><input name="target_term" required value="${this.attr(value?.target_term ?? '')}"></label><label><span>别名（用 | 分隔）</span><input name="aliases_text" value="${this.attr(value?.aliases.join('|') ?? '')}"></label><label><span>优先级</span><input name="priority" type="number" min="0" max="1000" value="${value?.priority ?? 100}"></label>${this.statusField(value?.status)}`, async (data) => { const body = { source_term: data.get('source_term'), target_term: data.get('target_term'), aliases: String(data.get('aliases_text') ?? '').split('|').map((item) => item.trim()).filter(Boolean), priority: Number(data.get('priority')), status: data.get('status') }; await apiRequest(value ? `/api/admin/terminology-entries/${value.id}` : `/api/admin/terminology-dictionaries/${this.selectedId}/entries`, { method: value ? 'PATCH' : 'POST', body: JSON.stringify(body) }); await this.loadEntries(); this.render(); }); }

  private blockedDialog(value?: BlockedWord) { this.openDialog(value ? '编辑屏蔽词' : '新增屏蔽词', `<label><span>屏蔽词</span><input name="word" required value="${this.attr(value?.word ?? '')}"></label><label><span>替换文本</span><input name="replacement" value="${this.attr(value?.replacement ?? '***')}"></label><label><span>匹配方式</span><select name="match_mode"><option value="substring" ${value?.match_mode !== 'word' ? 'selected' : ''}>包含匹配</option><option value="word" ${value?.match_mode === 'word' ? 'selected' : ''}>完整词匹配</option></select></label><label class="checkbox-row"><input name="case_sensitive" type="checkbox" ${value?.case_sensitive ? 'checked' : ''}><span>区分大小写</span></label><label><span>备注</span><input name="note" value="${this.attr(value?.note ?? '')}"></label>${this.statusField(value?.status)}`, async (data) => { const body = { word: data.get('word'), replacement: data.get('replacement'), match_mode: data.get('match_mode'), case_sensitive: data.get('case_sensitive') === 'on', note: data.get('note'), status: data.get('status') }; await apiRequest(value ? `/api/admin/blocked-words/${value.id}` : '/api/admin/blocked-words', { method: value ? 'PATCH' : 'POST', body: JSON.stringify(body) }); await this.load(); }); }

  private openDialog(title: string, fields: string, save: (data: FormData) => Promise<void>) {
    const dialog = this.root?.querySelector<HTMLDialogElement>('.lexicon-dialog'); if (!dialog) return;
    dialog.innerHTML = `<form method="dialog"><div class="dialog-heading"><div><small>LANGUAGE POLICY</small><h2>${title}</h2></div><button class="icon-button" value="cancel" title="关闭"><i data-lucide="x"></i></button></div>${fields}<p class="form-error" role="alert"></p><button class="primary-command" type="submit" value="save">保存</button></form>`;
    const form = dialog.querySelector('form')!; form.addEventListener('submit', async (event) => { event.preventDefault(); const submit = form.querySelector<HTMLButtonElement>('.primary-command')!; submit.disabled = true; try { await save(new FormData(form)); dialog.close(); this.onMessage('词库设置已保存'); } catch (error) { form.querySelector('.form-error')!.textContent = this.message(error); } finally { submit.disabled = false; } });
    refreshIcons(dialog); dialog.showModal();
  }

  private async remove(action: string, id: string) { try { const path = action === 'delete-entry' ? `/api/admin/terminology-entries/${id}` : action === 'delete-dictionary' ? `/api/admin/terminology-dictionaries/${id}` : `/api/admin/blocked-words/${id}`; await apiRequest(path, { method: 'DELETE' }); if (action === 'delete-dictionary') this.selectedId = ''; await this.load(); this.onMessage('记录已软删除并保留历史'); } catch (error) { this.onError(this.message(error)); } }

  private async importFile(event: Event) { const input = event.currentTarget as HTMLInputElement; const file = input.files?.[0]; if (!file) return; try { const result = await apiRequest<{ imported: number; errors: string[] }>(input.dataset.kind === 'import-entries' ? `/api/admin/terminology-dictionaries/${this.selectedId}/import` : '/api/admin/blocked-words/import', { method: 'POST', body: JSON.stringify({ content: await file.text() }) }); await this.load(); this.onMessage(`已导入 ${result.imported} 条${result.errors.length ? `，${result.errors.length} 条未导入：${result.errors.slice(0, 3).join('；')}` : ''}`); } catch (error) { this.onError(this.message(error)); } }

  private statusField(status = 'active') { return `<label><span>状态</span><select name="status"><option value="active" ${status === 'active' ? 'selected' : ''}>启用</option><option value="disabled" ${status === 'disabled' ? 'selected' : ''}>停用</option></select></label>`; }
  private escape(value: string) { const node = document.createElement('span'); node.textContent = value; return node.innerHTML; }
  private attr(value: string) { return this.escape(value).replaceAll('"', '&quot;'); }
  private message(error: unknown) { return error instanceof Error ? error.message : '操作失败'; }
}
