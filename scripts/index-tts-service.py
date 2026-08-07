#!/usr/bin/env python3
"""Project-owned HTTP boundary around the official IndexTTS2 inference class."""

import argparse
import os
import tempfile
import threading
from pathlib import Path

from fastapi import FastAPI, File, Form, Header, HTTPException, UploadFile
from fastapi.responses import Response
from huggingface_hub import snapshot_download

from indextts.infer_v2 import IndexTTS2


def parse_voice_map(value: str) -> dict[str, Path]:
    result: dict[str, Path] = {}
    for entry in value.split(","):
        if "=" not in entry:
            continue
        key, path = entry.split("=", 1)
        if key.strip() and path.strip():
            result[key.strip().upper()] = Path(path.strip()).expanduser().resolve()
    return result


parser = argparse.ArgumentParser()
parser.add_argument("--host", default="127.0.0.1")
parser.add_argument("--port", type=int, default=18084)
parser.add_argument("--model-dir", type=Path, required=True)
parser.add_argument("--source-dir", type=Path, required=True)
parser.add_argument("--fp16", action="store_true")
args = parser.parse_args()

model_dir = args.model_dir.resolve()
if not (model_dir / "config.yaml").is_file():
    snapshot_download("IndexTeam/IndexTTS-2", local_dir=model_dir)

default_reference = Path(
    os.environ.get(
        "TTS_INDEX_DEFAULT_REFERENCE_AUDIO",
        str(args.source_dir / "examples" / "voice_01.wav"),
    )
).expanduser().resolve()
voice_map = parse_voice_map(os.environ.get("TTS_INDEX_VOICE_MAP", ""))
api_key = os.environ.get("TTS_INDEX_API_KEY", "").strip()

engine = IndexTTS2(
    cfg_path=str(model_dir / "config.yaml"),
    model_dir=str(model_dir),
    use_fp16=args.fp16,
    use_cuda_kernel=False,
    use_deepspeed=False,
)
inference_lock = threading.Lock()
app = FastAPI(title="Voice Elf IndexTTS2 Sidecar", docs_url=None, redoc_url=None)


def authorize(authorization: str | None) -> None:
    if api_key and authorization != f"Bearer {api_key}":
        raise HTTPException(status_code=401, detail="invalid API key")


@app.get("/health")
def health(authorization: str | None = Header(default=None)) -> dict[str, str]:
    authorize(authorization)
    return {"status": "ok", "engine": "index-tts2"}


@app.post("/v1/tts")
def synthesize(
    text: str = Form(...),
    language: str = Form("zh"),
    voice: str = Form("F1"),
    reference_audio: UploadFile | None = File(default=None),
    authorization: str | None = Header(default=None),
) -> Response:
    authorize(authorization)
    normalized_text = text.strip()
    if not normalized_text or len(normalized_text) > 5_000:
        raise HTTPException(status_code=400, detail="text length must be between 1 and 5000")

    reference_path = voice_map.get(voice.upper(), default_reference)
    temporary_reference: Path | None = None
    output_path: Path | None = None
    try:
        if reference_audio is not None:
            suffix = Path(reference_audio.filename or "reference.wav").suffix or ".wav"
            with tempfile.NamedTemporaryFile(suffix=suffix, delete=False) as temporary:
                temporary.write(reference_audio.file.read())
                temporary_reference = Path(temporary.name)
                reference_path = temporary_reference
        if not reference_path.is_file():
            raise HTTPException(
                status_code=503,
                detail=f"reference audio is not configured for voice {voice}",
            )
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as output:
            output_path = Path(output.name)
        with inference_lock:
            engine.infer(
                spk_audio_prompt=str(reference_path),
                text=normalized_text,
                output_path=str(output_path),
                verbose=False,
            )
        audio = output_path.read_bytes()
        if len(audio) <= 44:
            raise HTTPException(status_code=502, detail="IndexTTS2 returned empty audio")
        return Response(
            content=audio,
            media_type="audio/wav",
            headers={
                "Cache-Control": "no-store",
                "X-TTS-Engine": "index-tts2",
                "X-TTS-Language": language,
            },
        )
    finally:
        if temporary_reference is not None:
            temporary_reference.unlink(missing_ok=True)
        if output_path is not None:
            output_path.unlink(missing_ok=True)


if __name__ == "__main__":
    import uvicorn

    uvicorn.run(app, host=args.host, port=args.port, log_level="info")
