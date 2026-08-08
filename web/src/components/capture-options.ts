import { refreshIcons } from './icons';

export interface CaptureOptionValues {
  microphone: boolean;
  systemAudio: boolean;
  noiseSuppression: boolean;
  echoCancellation: boolean;
}

export interface CaptureLanguageOption {
  sourceLabel: string;
  targetLabel: string;
  editable: boolean;
  onOpen: () => void;
}

let captureOptionsSequence = 0;

export class CaptureOptions {
  readonly element: HTMLElement;
  private readonly popover: HTMLElement;
  private readonly popoverId = `capture-options-${++captureOptionsSequence}`;

  constructor(
    initial: CaptureOptionValues,
    private readonly systemAudioAvailable: boolean,
    private readonly onChange: (values: CaptureOptionValues) => void,
    private readonly language: CaptureLanguageOption,
  ) {
    this.element = document.createElement('div');
    this.element.className = 'capture-options-control';
    this.element.innerHTML = `
      <button class="capture-options-trigger icon-button" type="button" aria-expanded="false" aria-haspopup="dialog" aria-controls="${this.popoverId}" title="输入与语言设置" aria-label="输入与语言设置">
        <i data-lucide="sliders-horizontal"></i>
      </button>
      <span class="capture-options-trigger-label">输入设置</span>
    `;
    this.popover = document.createElement('section');
    this.popover.id = this.popoverId;
    this.popover.className = 'capture-options-popover';
    this.popover.hidden = true;
    this.popover.setAttribute('role', 'dialog');
    this.popover.setAttribute('aria-label', '音频输入与处理');
    this.popover.innerHTML = `
      <header class="capture-options-heading">
        <div><span>CAPTURE</span><h2>录音设置</h2></div>
        <output class="capture-mode-status" aria-live="polite"></output>
      </header>
      <button class="capture-language-option" type="button" ${language.editable ? '' : 'disabled'}>
        <span class="capture-source-icon"><i data-lucide="languages"></i></span>
        <span><strong>翻译语言</strong><small><span class="capture-language-source"></span><i data-lucide="arrow-right"></i><span class="capture-language-target"></span></small></span>
        <i data-lucide="chevron-right"></i>
      </button>
      <fieldset class="capture-source-options">
        <legend>输入来源</legend>
        <label class="capture-source-option">
          <input type="checkbox" data-capture-option="microphone">
          <span class="capture-source-icon"><i data-lucide="mic"></i></span>
          <span><strong>麦克风录音</strong><small>现场发言与环境声音</small></span>
          <span class="capture-source-check"><i data-lucide="check"></i></span>
        </label>
        <label class="capture-source-option${systemAudioAvailable ? '' : ' is-unavailable'}">
          <input type="checkbox" data-capture-option="systemAudio" ${systemAudioAvailable ? '' : 'disabled'}>
          <span class="capture-source-icon"><i data-lucide="volume-2"></i></span>
          <span><strong>系统内录</strong><small>${systemAudioAvailable ? '标签页、窗口或设备播放声' : '当前浏览器或设备不可用'}</small></span>
          <span class="capture-source-check"><i data-lucide="check"></i></span>
        </label>
      </fieldset>
      <fieldset class="capture-processing-options">
        <legend>麦克风处理</legend>
        <label class="capture-processing-option">
          <span><strong>系统降噪</strong><small>抑制持续背景噪声</small></span>
          <input type="checkbox" role="switch" data-capture-option="noiseSuppression">
          <span class="capture-processing-track" aria-hidden="true"></span>
        </label>
        <label class="capture-processing-option">
          <span><strong>回声消除</strong><small>减少扬声器回采</small></span>
          <input type="checkbox" role="switch" data-capture-option="echoCancellation">
          <span class="capture-processing-track" aria-hidden="true"></span>
        </label>
      </fieldset>
      <p class="capture-option-message" role="status" aria-live="polite"></p>
    `;
    document.body.append(this.popover);
    this.setValues(initial);
    this.setLanguages(language.sourceLabel, language.targetLabel, language.editable);
    this.element.querySelector('button')!.addEventListener('click', this.togglePopover);
    this.popover.querySelector('.capture-language-option')!.addEventListener('click', this.openLanguages);
    this.popover.addEventListener('change', this.handleChange);
    document.addEventListener('pointerdown', this.handleOutsidePointer);
    document.addEventListener('keydown', this.handleKeydown);
    window.addEventListener('resize', this.closePopover);
    document.addEventListener('scroll', this.closePopover, true);
    refreshIcons(this.element);
    refreshIcons(this.popover);
  }

  values(): CaptureOptionValues {
    return {
      microphone: this.input('microphone').checked,
      systemAudio: this.input('systemAudio').checked,
      noiseSuppression: this.input('noiseSuppression').checked,
      echoCancellation: this.input('echoCancellation').checked,
    };
  }

  setValues(values: CaptureOptionValues) {
    this.input('microphone').checked = values.microphone;
    this.input('systemAudio').checked = this.systemAudioAvailable && values.systemAudio;
    this.input('noiseSuppression').checked = values.noiseSuppression;
    this.input('echoCancellation').checked = values.echoCancellation;
    this.syncState();
  }

  setRecording(recording: boolean) {
    this.popover.classList.toggle('is-recording', recording);
    this.syncState();
  }

  setLanguages(sourceLabel: string, targetLabel: string, editable = this.language.editable) {
    this.popover.querySelector<HTMLElement>('.capture-language-source')!.textContent = sourceLabel;
    this.popover.querySelector<HTMLElement>('.capture-language-target')!.textContent = targetLabel;
    const button = this.popover.querySelector<HTMLButtonElement>('.capture-language-option')!;
    button.disabled = !editable;
    button.title = editable ? '修改房间翻译语言' : '仅房主可修改翻译语言';
  }

  destroy() {
    this.element.querySelector('button')?.removeEventListener('click', this.togglePopover);
    this.popover.querySelector('.capture-language-option')?.removeEventListener('click', this.openLanguages);
    this.popover.removeEventListener('change', this.handleChange);
    document.removeEventListener('pointerdown', this.handleOutsidePointer);
    document.removeEventListener('keydown', this.handleKeydown);
    window.removeEventListener('resize', this.closePopover);
    document.removeEventListener('scroll', this.closePopover, true);
    this.popover.remove();
  }

  private input(name: keyof CaptureOptionValues) {
    return this.popover.querySelector<HTMLInputElement>(`[data-capture-option="${name}"]`)!;
  }

  private handleChange = () => {
    this.syncState();
    this.onChange(this.values());
  };

  private syncState() {
    const values = this.values();
    const microphoneProcessingDisabled = !values.microphone;
    this.input('microphone').disabled = false;
    this.input('systemAudio').disabled = !this.systemAudioAvailable;
    this.input('noiseSuppression').disabled = microphoneProcessingDisabled;
    this.input('echoCancellation').disabled = microphoneProcessingDisabled;
    const mode = values.microphone && values.systemAudio
      ? '混合输入'
      : values.microphone
        ? '麦克风'
        : values.systemAudio
          ? '系统音频'
          : '未选择';
    this.popover.querySelector<HTMLOutputElement>('.capture-mode-status')!.textContent = mode;
    this.popover.querySelector<HTMLElement>('.capture-option-message')!.textContent =
      values.microphone || values.systemAudio ? '' : '至少选择一个输入来源';
  }

  private togglePopover = () => {
    if (this.popover.hidden) this.openPopover();
    else this.closePopover();
  };

  private openLanguages = () => {
    if (!this.language.editable) return;
    this.closePopover();
    this.language.onOpen();
  };

  private openPopover() {
    this.popover.hidden = false;
    this.popover.style.maxHeight = '';
    const trigger = this.element.getBoundingClientRect();
    const margin = 12;
    const gap = 8;
    let panel = this.popover.getBoundingClientRect();
    const left = Math.max(
      margin,
      Math.min(trigger.right - panel.width, window.innerWidth - panel.width - margin),
    );
    const spaceAbove = Math.max(0, trigger.top - margin - gap);
    const spaceBelow = Math.max(0, window.innerHeight - trigger.bottom - margin - gap);
    const placeAbove = spaceAbove >= panel.height || spaceAbove > spaceBelow;
    const availableHeight = placeAbove ? spaceAbove : spaceBelow;
    this.popover.style.maxHeight = `${Math.floor(availableHeight)}px`;
    panel = this.popover.getBoundingClientRect();
    const top = placeAbove ? trigger.top - panel.height - gap : trigger.bottom + gap;
    this.popover.style.left = `${Math.round(left)}px`;
    this.popover.style.top = `${Math.round(top)}px`;
    this.element.querySelector('button')!.setAttribute('aria-expanded', 'true');
    this.input('microphone').focus();
  }

  private closePopover = () => {
    if (this.popover.hidden) return;
    this.popover.hidden = true;
    this.element.querySelector('button')?.setAttribute('aria-expanded', 'false');
  };

  private handleOutsidePointer = (event: PointerEvent) => {
    const target = event.target as Node;
    if (!this.element.contains(target) && !this.popover.contains(target)) this.closePopover();
  };

  private handleKeydown = (event: KeyboardEvent) => {
    if (event.key !== 'Escape' || this.popover.hidden) return;
    event.preventDefault();
    this.closePopover();
    this.element.querySelector<HTMLButtonElement>('button')?.focus();
  };
}
