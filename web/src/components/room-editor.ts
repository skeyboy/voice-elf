import { apiRequest, type RoomInput, type RoomSummary } from '../api';
import { languageOptions } from '../shared/languages';
import { refreshIcons } from './icons';

export class RoomEditor {
  private readonly dialog: HTMLDialogElement;
  private room: RoomSummary | null = null;

  constructor(private readonly onSaved: (room: RoomSummary) => void) {
    this.dialog = document.createElement('dialog');
    this.dialog.className = 'room-editor';
    this.dialog.innerHTML = `
      <form>
        <div class="dialog-heading">
          <div><small>ROOM SETTINGS</small><h2>新建房间</h2></div>
          <button class="icon-button close-editor" type="button" title="关闭" aria-label="关闭"><i data-lucide="x"></i></button>
        </div>
        <label><span>房间名称</span><input name="name" maxlength="120" required></label>
        <div class="editor-language-pair">
          <label><span>源语言</span><select name="source_language">${languageOptions(true)}</select></label>
          <label><span>目标语言</span><select name="target_language">${languageOptions(false)}</select></label>
        </div>
        <label><span>最长断句（秒）</span><input name="max_utterance_seconds" type="number" min="5" max="120" step="1" required></label>
        <p class="form-error" role="alert"></p>
        <button class="primary-command" type="submit">保存房间</button>
      </form>
    `;
    this.dialog.querySelector('.close-editor')?.addEventListener('click', () => this.dialog.close());
    this.dialog.querySelector('form')?.addEventListener('submit', (event) => void this.save(event));
    document.body.append(this.dialog);
    refreshIcons(this.dialog);
  }

  open(room?: RoomSummary) {
    this.room = room ?? null;
    this.dialog.querySelector('h2')!.textContent = room ? '编辑房间' : '新建房间';
    this.dialog.querySelector<HTMLInputElement>('[name="name"]')!.value = room?.name ?? '';
    this.dialog.querySelector<HTMLSelectElement>('[name="source_language"]')!.value =
      room?.source_language ?? 'auto';
    this.dialog.querySelector<HTMLSelectElement>('[name="target_language"]')!.value =
      room?.target_language ?? 'zh';
    this.dialog.querySelector<HTMLInputElement>('[name="max_utterance_seconds"]')!.value = String(
      room?.max_utterance_seconds ?? 20,
    );
    this.dialog.querySelector('.form-error')!.textContent = '';
    this.dialog.showModal();
  }

  destroy() {
    this.dialog.remove();
  }

  private async save(event: SubmitEvent) {
    event.preventDefault();
    const form = event.currentTarget as HTMLFormElement;
    const values = new FormData(form);
    const input: RoomInput = {
      name: String(values.get('name') ?? ''),
      source_language: String(values.get('source_language') ?? 'auto'),
      target_language: String(values.get('target_language') ?? 'zh'),
      max_utterance_seconds: Number(values.get('max_utterance_seconds') ?? 20),
    };
    const errorElement = form.querySelector<HTMLElement>('.form-error')!;
    const submit = form.querySelector<HTMLButtonElement>('[type="submit"]')!;
    errorElement.textContent = '';
    submit.disabled = true;
    try {
      const saved = await apiRequest<RoomSummary>(
        this.room ? `/api/rooms/${this.room.id}` : '/api/rooms',
        { method: this.room ? 'PATCH' : 'POST', body: JSON.stringify(input) },
      );
      this.dialog.close();
      this.onSaved(saved);
    } catch (error) {
      errorElement.textContent = error instanceof Error ? error.message : '无法保存房间';
    } finally {
      submit.disabled = false;
    }
  }
}
