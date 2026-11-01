"""Publish repository references, images and example notebooks without duplicate sources."""

from pathlib import Path

from mkdocs.structure.files import File

ROOT = Path(__file__).resolve().parent.parent


def on_page_markdown(markdown, **_kwargs):
    # Source links work on GitHub; generated references use their site URLs.
    """Rewrite repository-relative references to the matching generated documentation-site locations."""
    for source, destination in {
        "../themes/README.md": "theme-format.md",
        "../docs/syntax-themes.md": "syntax-themes.md",
        "../src/sandbox_policy.json": "https://github.com/pbenner/rode/blob/main/src/sandbox_policy.json",
    }.items():
        markdown = markdown.replace(f"]({source})", f"]({destination})")
    return markdown.replace("](../assets/", "](images/").replace("](../examples/", "](examples/")


def on_files(files, config):
    """Export canonical theme definitions, image assets and example notebooks into the MkDocs file set."""
    files.append(
        File.generated(config, "theme-format.md", content=(ROOT / "themes/README.md").read_text())
    )
    for definition in sorted((ROOT / "themes").glob("*.json")):
        files.append(File.generated(config, definition.name, abs_src_path=str(definition)))
    for image in sorted((ROOT / "assets").glob("*.png")):
        files.append(File.generated(config, f"images/{image.name}", abs_src_path=str(image)))
    for notebook in sorted((ROOT / "examples").glob("*.ipynb")):
        files.append(
            File.generated(config, f"examples/{notebook.name}", abs_src_path=str(notebook))
        )
    return files
