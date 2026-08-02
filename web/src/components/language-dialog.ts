import { languageOptions } from '../shared/languages';
import { refreshIcons } from './icons';

export class LanguageDialog {
  private readonly dialog: HTMLDialogElement;

  constructor(
    private readonly onSave: (source: string, target: string) => Promise<void>,
    private readonly onError: (message: string) => void,
  ) {
    this.dialog = document.createElement('dialog');
    this.dialog.className = 'language-dialog';
    this.dialog.innerHTML = `
      <form>
        <div class="dialog-heading">
          <div><small>LANGUAGE DIRECTION</small><h2>翻译语言</h2></div>
          <button class="icon-button close-language-dialog" type="button" title="关闭" aria-label="关闭"><i data-lucide="x"></i></button>
        </div>
        <div class="language-dialog-pair">
          <label><span>源语言</span><select name="source">${languageOptions(true)}</select></label>
          <span class="language-direction-icon" aria-hidden="true"><i data-lucide="arrow-left-right"></i></span>
          <label><span>目标语言</span><select name="target">${languageOptions(false)}</select></label>
        </div>
        <button class="primary-command" type="submit">应用语言</button>
      </form>
    `;
    this.dialog.querySelector('.close-language-dialog')?.addEventListener('click', () => this.dialog.close());
    this.dialog.querySelector('form')?.addEventListener('submit', (event) => void this.save(event));
    document.body.append(this.dialog);
    refreshIcons(this.dialog);
  }

  open(source: string, target: string) {
    this.dialog.querySelector<HTMLSelectElement>('[name="source"]')!.value = source;
    this.dialog.querySelector<HTMLSelectElement>('[name="target"]')!.value = target;
    this.dialog.showModal();
  }

  destroy() {
    this.dialog.remove();
  }

  private async save(event: SubmitEvent) {
    event.preventDefault();
    const submit = this.dialog.querySelector<HTMLButtonElement>('[type="submit"]')!;
    submit.disabled = true;
    try {
      await this.onSave(
        this.dialog.querySelector<HTMLSelectElement>('[name="source"]')!.value,
        this.dialog.querySelector<HTMLSelectElement>('[name="target"]')!.value,
      );
      this.dialog.close();
    } catch (error) {
      this.onError(error instanceof Error ? error.message : '无法更新翻译语言');
    } finally {
      submit.disabled = false;
    }
  }
}
