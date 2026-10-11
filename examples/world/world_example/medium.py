"""Medium contracts: the production language a universe package is rendered in.

Medium is a first-class package input (``universe.toml`` ``medium``). It enters
before concept compilation, not only at the final image prompt, because the
benchmark proved a compiler that speaks one medium overrules any renderer that
speaks another. Nothing in the semantic proposal is medium-specific.
"""

from __future__ import annotations

# ruff: noqa: E501
from dataclasses import dataclass
from typing import Final

from world_example.canonical import content_sha256


@dataclass(frozen=True, slots=True)
class MediumContract:
    medium_id: str
    display_name: str
    #: Told to the global-direction and entity-direction compilers.
    compile_guidance: str
    #: Prepended to every image prompt. Highest priority.
    render_block: str
    #: Appended to every image prompt.
    negative_block: str
    #: Told to the image reviewer as the definition of medium_fidelity.
    review_criteria: str
    #: Words that must not appear in compiled direction prose for this medium.
    forbidden_direction_terms: tuple[str, ...]

    def compile_digest(self) -> str:
        """Identity for the direction compilers: what they are told, and what refuses them.

        ``forbidden_direction_terms`` is not prose the compiler is shown, but it
        is a hard acceptance rule the compiled direction has to clear, so it
        belongs here: adding a term must re-run the directions it would now
        refuse, or the rule applies or not depending on what happens to be in
        the cache.
        """

        return content_sha256(
            "\n".join((self.compile_guidance, *self.forbidden_direction_terms)).encode("utf-8")
        )

    def render_digest(self) -> str:
        """Identity for the image nodes: every block that reaches an image prompt.

        The shared staging, weather, and output-form rules are part of the
        rendered instruction, so they belong to this digest even though they do
        not vary by medium; a change to any of them is a different picture.
        """

        return content_sha256(
            "\n\n".join(
                (
                    self.render_block,
                    SHARED_STAGING_RULE,
                    SHARED_LOCAL_WEATHER,
                    SHARED_OUTPUT_FORM,
                    self.negative_block,
                )
            ).encode("utf-8")
        )

    def review_digest(self) -> str:
        """Identity for the review nodes alone. Includes the name they are told.

        Kept out of ``render_digest`` deliberately. The spike hashed compile,
        render, negative, and review prose into one digest bound to every
        direction, image, and review node, so recalibrating the reviewer
        re-billed the whole gallery — about USD 11 to change how an image was
        judged rather than how it was drawn.
        """

        return content_sha256("\n".join((self.display_name, self.review_criteria)).encode("utf-8"))


SHARED_STAGING_RULE: Final = """CONSEQUENTIAL IN-WORLD STAGING: manifest the primary entity through consequential physical action, use, occupation, or environmental change at human scale. For a collective, idea, system, or event, show people, structures, tools, weather, water, vegetation, or terrain materially doing, undergoing, enforcing, or responding to the entity's consequences. Never substitute an explanatory tableau: no one studies or points at a flat surface; no papers, plans, notes, cards, pages, arranged tokens, miniature layouts, maps, schematics, charts, or props whose identity depends on marks or text. A closed, blank, edge-on book may remain a subordinate carried object."""

SHARED_OUTPUT_FORM: Final = """OUTPUT FORM: exactly one fresh text-to-image result with zero image references and zero masks; one primary entity in one continuous in-world scene and one panel. Render no title, readable words, numbers, labels, captions, UI, legends, arrows, watermark, or signature. Render no diagram, map, timeline, graph, atlas, sheet, kit, blueprint, infographic, montage, collage, split screen, turnaround, variant grid, or standalone symbol presentation. Incidental non-semantic surface marks on machinery or fabric are acceptable when nothing can be read from them."""

SHARED_LOCAL_WEATHER: Final = """Weather, time of day, wetness, and mood are scene-local facts from the sealed plan, never a global universe style. A dry or clear scene must remain visibly dry and clear. Keep whites and grays neutral. No global yellow, amber, sepia, tobacco, olive, aged-paper, monochrome, or teal-orange grade."""


ANIME_2D: Final = MediumContract(
    medium_id="anime_2d",
    display_name="Original hand-drawn 2D Japanese theatrical anime feature",
    compile_guidance="""MEDIUM: original hand-drawn 2D Japanese theatrical anime feature animation. Describe subjects as drawn characters and painted layouts: clean tapered cleanup lines, flat local color, one hard-edged shadow shape, at most one small flat highlight, silhouette-first background paintings built from four to six large value groups, controlled brush vocabulary, and detail that falls off sharply with distance. Describe performance as animation acting: posture, weight, timing, expression, cloth response. Never describe photography, lenses, film stock, skin pores, depth of field, or 3D rendering. Do not name any studio, artist, or franchise.""",
    render_block="""IMMUTABLE RENDER MEDIUM, highest priority: a production frame from an original hand-drawn Japanese theatrical anime feature. Apply the same visibly 2D production language to the primary subject, foreground objects, weather, vegetation, architecture, and distant scenery.

Build the image from broad opaque color shapes and economical tapered cleanup lines. Characters and primary objects use flat local color, one hard-edged shadow shape, and at most one small flat highlight shape. Do not softly model volume.

Backgrounds are silhouette-first paintings made from four to six large value groups, simplified material planes, and a small controlled brush vocabulary. Detail decreases sharply with distance. Native resolution is for clean edges, stable anatomy, readable mechanics, and theatrical print clarity, not microtexture.

Preserve richness through one crisp subject-and-action detail island and at most one selectively articulated environmental identity cluster per depth plane; merge repeated bark marks, leaves, masonry units, scaffold members, and ropes into broad grouped shapes. When the sealed scene calls for rain or wet surfaces, moisture darkens flat local color; use at most three short discontinuous cool highlight strokes near the action. Surfaces remain matte.

Create depth through overlap, value grouping, reduced saturation, and fewer edges, not photographic optics.""",
    negative_block="""TERMINAL NEGATIVE BLOCK: do not render as photography, live action, semi-real game concept art, 3D/CG, PBR, photobash, or realistic matte painting. No glossy wet-surface response, volumetric lighting, HDR, photographic depth of field, film grain, microtexture, pseudo-detail, repeated generative patterns, or an anime character over a realistic environment.""",
    review_criteria="""medium_fidelity passes only when subject, foreground objects, weather, architecture, vegetation, and distant scenery share one visibly hand-drawn 2D anime production language with flat local color, hard-edged shadow shapes, and painted silhouette-first backgrounds. Fail photographic rendering, 3D/CG or PBR surfaces, photobash, an anime subject over a realistic environment, uniform microdetail across depth planes, or glossy specular wetness.""",
    forbidden_direction_terms=(
        "photograph",
        "photographic",
        "photoreal",
        "live-action",
        "live action",
        "film stock",
        "skin pores",
        "3d render",
    ),
)

LIVE_ACTION: Final = MediumContract(
    medium_id="live_action",
    display_name="Photographed live-action theatrical feature",
    compile_guidance="""MEDIUM: photographed live-action theatrical feature production. Describe subjects as real performers in practical wardrobe, built sets or real locations, physical props with credible construction and wear, motivated practical lighting, real optics, and restrained invisible effects. Describe performance as physical acting: weight, breath, gaze, cloth response. Never describe drawing, animation, cel shading, painted backgrounds, line art, or illustration. Do not name any performer, studio, or franchise.""",
    render_block="""IMMUTABLE RENDER MEDIUM, highest priority: a newly photographed live-action production still from a major theatrical feature, never an illustration, poster, or reproduction of promotional art. Build a complete physical moment using believable performers, practical sets, real costumes, weathered props, full-scale or miniature construction, and restrained invisible visual effects. Exact likeness to any real performer is neither requested nor required.

Use clean native high-resolution photographic detail: coherent anatomy, pores without waxiness, woven fabric, machined edges, fasteners, controlled wear, optically believable atmosphere, and motivated practical lighting. Preserve one crisp subject-and-action detail island and at most one selectively articulated environmental identity cluster per depth plane; do not spread uniform microdetail, repeated scratches, or generative noise across every surface.

Create depth through real blocking, lens perspective, atmosphere, overlap, and lighting. The finished frame must survive close inspection as a plausible live-action film image, not as AI-textured concept art.""",
    negative_block="""TERMINAL NEGATIVE BLOCK: do not render as anime, animation, drawing, painting, poster art, game concept art, comic, 3D render, glossy CG, PBR showcase, photobash, plastic miniature display, or fake documentary still. No waxy skin, smeared faces, malformed hands, synthetic fabric, random surface glyphs, repeated generative patterns, uniform pseudo-detail, excessive bloom, artificial HDR, extreme depth of field, or film-damage overlay.""",
    review_criteria="""medium_fidelity passes only when the image reads as a photographed live-action production still: real performers, practical wardrobe and construction, motivated lighting, credible optics, coherent anatomy. Fail illustration, animation, painterly or cel rendering, glossy CG or PBR surfaces, waxy skin, photobash seams, or AI-textured concept-art finish.""",
    forbidden_direction_terms=(
        "anime",
        "animation",
        "cel shading",
        "cel-shaded",
        "line art",
        "linework",
        "painted background",
        "illustration",
        "drawn character",
    ),
)

MEDIA: Final[dict[str, MediumContract]] = {
    ANIME_2D.medium_id: ANIME_2D,
    LIVE_ACTION.medium_id: LIVE_ACTION,
}


def medium_contract(medium_id: str) -> MediumContract:
    try:
        return MEDIA[medium_id]
    except KeyError as error:
        raise ValueError(f"unknown medium {medium_id!r}; known: {sorted(MEDIA)}") from error


def forbidden_terms_present(medium: MediumContract, text: str) -> list[str]:
    lowered = text.lower()
    return [term for term in medium.forbidden_direction_terms if term in lowered]
