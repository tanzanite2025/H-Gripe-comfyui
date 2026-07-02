"""Tiny random-weight diffusers snapshots for the gated real-inference tests.

Mirrors the synthesised-ONNX lanes: no hub download, just the smallest
components each pipeline accepts, saved with ``save_pretrained`` so the real
``from_pretrained`` -> denoise-loop -> VAE-decode path runs in seconds on CPU.
The hand-written CLIP BPE vocab (and the file-less ByT5 tokenizer for Flux)
keep the snapshots hermetic.

Only imported by tests already gated on ``torch`` / ``diffusers`` /
``transformers`` being importable, so the heavy imports live inside the
builders.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any


def tiny_clip_tokenizer(work_dir: Path) -> Any:
    """A hermetic CLIP BPE tokenizer: specials + each ASCII letter as a
    mid-word and an end-of-word token, so any lowercase prompt tokenises
    without merges."""
    from transformers import CLIPTokenizer

    vocab: dict[str, int] = {"<|startoftext|>": 0, "<|endoftext|>": 1}
    for ch in "abcdefghijklmnopqrstuvwxyz":
        vocab[ch] = len(vocab)
        vocab[ch + "</w>"] = len(vocab)
    vocab_file = work_dir / "vocab.json"
    merges_file = work_dir / "merges.txt"
    vocab_file.write_text(json.dumps(vocab), encoding="utf-8")
    merges_file.write_text("#version: 0.2\n", encoding="utf-8")
    return CLIPTokenizer(str(vocab_file), str(merges_file), model_max_length=77)


def _tiny_vae() -> Any:
    from diffusers import AutoencoderKL

    return AutoencoderKL(
        block_out_channels=(32, 64),
        in_channels=3,
        out_channels=3,
        down_block_types=("DownEncoderBlock2D", "DownEncoderBlock2D"),
        up_block_types=("UpDecoderBlock2D", "UpDecoderBlock2D"),
        latent_channels=4,
    )


def _tiny_clip_text_encoder(**overrides: Any) -> Any:
    from transformers import CLIPTextConfig, CLIPTextModel

    config = dict(
        bos_token_id=0,
        eos_token_id=2,
        hidden_size=32,
        intermediate_size=37,
        layer_norm_eps=1e-05,
        num_attention_heads=4,
        num_hidden_layers=5,
        pad_token_id=1,
        vocab_size=1000,
    )
    config.update(overrides)
    return CLIPTextModel(CLIPTextConfig(**config))


def save_tiny_sd_inpaint(path: Path) -> None:
    """Tiny ``StableDiffusionInpaintPipeline`` snapshot (9-channel inpaint UNet)."""
    import torch
    from diffusers import PNDMScheduler, StableDiffusionInpaintPipeline, UNet2DConditionModel

    torch.manual_seed(0)
    unet = UNet2DConditionModel(
        block_out_channels=(32, 64),
        layers_per_block=2,
        sample_size=32,
        in_channels=9,  # 4 latent + 4 masked-latent + 1 mask: the inpaint UNet layout
        out_channels=4,
        down_block_types=("DownBlock2D", "CrossAttnDownBlock2D"),
        up_block_types=("CrossAttnUpBlock2D", "UpBlock2D"),
        cross_attention_dim=32,
    )
    pipe = StableDiffusionInpaintPipeline(
        unet=unet,
        vae=_tiny_vae(),
        text_encoder=_tiny_clip_text_encoder(),
        tokenizer=tiny_clip_tokenizer(path.parent),
        scheduler=PNDMScheduler(skip_prk_steps=True),
        safety_checker=None,
        feature_extractor=None,
        requires_safety_checker=False,
    )
    pipe.save_pretrained(str(path))


def save_tiny_sdxl_inpaint(path: Path) -> None:
    """Tiny ``StableDiffusionXLInpaintPipeline`` snapshot (dual text encoders)."""
    import torch
    from diffusers import EulerDiscreteScheduler, StableDiffusionXLInpaintPipeline
    from diffusers import UNet2DConditionModel
    from transformers import CLIPTextConfig, CLIPTextModelWithProjection

    torch.manual_seed(0)
    unet = UNet2DConditionModel(
        block_out_channels=(32, 64),
        layers_per_block=2,
        sample_size=32,
        in_channels=4,  # SDXL inpaint also accepts the 4-channel img2img UNet
        out_channels=4,
        down_block_types=("DownBlock2D", "CrossAttnDownBlock2D"),
        up_block_types=("CrossAttnUpBlock2D", "UpBlock2D"),
        use_linear_projection=True,
        addition_embed_type="text_time",
        addition_time_embed_dim=8,
        # 6 time-ids * 8 + the pooled text embedding (projection_dim=32)
        projection_class_embeddings_input_dim=80,
        # concat of the two text encoders' hidden sizes (32 + 32)
        cross_attention_dim=64,
    )
    text_encoder_2 = CLIPTextModelWithProjection(
        CLIPTextConfig(
            bos_token_id=0,
            eos_token_id=2,
            hidden_size=32,
            intermediate_size=37,
            layer_norm_eps=1e-05,
            num_attention_heads=4,
            num_hidden_layers=5,
            pad_token_id=1,
            vocab_size=1000,
            projection_dim=32,
        )
    )
    pipe = StableDiffusionXLInpaintPipeline(
        unet=unet,
        vae=_tiny_vae(),
        text_encoder=_tiny_clip_text_encoder(projection_dim=32),
        text_encoder_2=text_encoder_2,
        tokenizer=tiny_clip_tokenizer(path.parent),
        tokenizer_2=tiny_clip_tokenizer(path.parent),
        scheduler=EulerDiscreteScheduler(),
    )
    pipe.save_pretrained(str(path))


def save_tiny_flux_fill(path: Path) -> None:
    """Tiny ``FluxFillPipeline`` snapshot (flow-matching transformer)."""
    import torch
    from diffusers import AutoencoderKL, FluxFillPipeline, FluxTransformer2DModel
    from diffusers import FlowMatchEulerDiscreteScheduler
    from transformers import ByT5Tokenizer, T5Config, T5EncoderModel

    torch.manual_seed(0)
    # Flux packs 2x2 latent patches; with a 1-channel VAE a packed image token
    # is 4 channels, and Fill appends the packed masked-image latents (4) +
    # the packed mask (4), so the transformer sees 12 input channels.
    transformer = FluxTransformer2DModel(
        patch_size=1,
        in_channels=12,
        out_channels=4,
        num_layers=1,
        num_single_layers=1,
        attention_head_dim=16,
        num_attention_heads=2,
        joint_attention_dim=32,
        pooled_projection_dim=32,
        axes_dims_rope=(4, 4, 8),
    )
    vae = AutoencoderKL(
        block_out_channels=(32,),
        in_channels=3,
        out_channels=3,
        down_block_types=("DownEncoderBlock2D",),
        up_block_types=("UpDecoderBlock2D",),
        latent_channels=1,
        # Flux's latent (un)shift reads these from the VAE config.
        scaling_factor=1.0,
        shift_factor=0.0,
    )
    text_encoder_2 = T5EncoderModel(
        T5Config(
            vocab_size=384,  # ByT5: 256 bytes + specials, rounded up
            d_model=32,
            d_ff=37,
            d_kv=8,
            num_layers=2,
            num_heads=4,
        )
    )
    pipe = FluxFillPipeline(
        scheduler=FlowMatchEulerDiscreteScheduler(),
        # Flux reads the CLIP encoder's ``pooler_output``, so the plain
        # CLIPTextModel (not the projection variant); its hidden size must
        # match the transformer's pooled_projection_dim.
        text_encoder=_tiny_clip_text_encoder(),
        tokenizer=tiny_clip_tokenizer(path.parent),
        text_encoder_2=text_encoder_2,
        tokenizer_2=ByT5Tokenizer(),  # file-less: keeps the snapshot hermetic
        vae=vae,
        transformer=transformer,
    )
    pipe.save_pretrained(str(path))


def save_tiny_sd_img2img(path: Path) -> None:
    """Tiny ``StableDiffusionImg2ImgPipeline`` snapshot for the diffusion SR
    engines (CCSR / SupIR load snapshots generically via ``DiffusionPipeline``
    and call them with the shared ``pipe(prompt, image=...)`` convention)."""
    import torch
    from diffusers import PNDMScheduler, StableDiffusionImg2ImgPipeline, UNet2DConditionModel

    torch.manual_seed(0)
    unet = UNet2DConditionModel(
        block_out_channels=(32, 64),
        layers_per_block=2,
        sample_size=32,
        in_channels=4,
        out_channels=4,
        down_block_types=("DownBlock2D", "CrossAttnDownBlock2D"),
        up_block_types=("CrossAttnUpBlock2D", "UpBlock2D"),
        cross_attention_dim=32,
    )
    pipe = StableDiffusionImg2ImgPipeline(
        unet=unet,
        vae=_tiny_vae(),
        text_encoder=_tiny_clip_text_encoder(),
        tokenizer=tiny_clip_tokenizer(path.parent),
        scheduler=PNDMScheduler(skip_prk_steps=True),
        safety_checker=None,
        feature_extractor=None,
        requires_safety_checker=False,
    )
    pipe.save_pretrained(str(path))
