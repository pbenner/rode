"""Publish repository references, images and example notebooks without duplicate sources."""

from pathlib import Path

from mkdocs.structure.files import File

ROOT = Path(__file__).resolve().parent.parent


def on_page_markdown(markdown, **_kwargs):
    # Source links work on GitHub; generated references use their site URLs.
    """Rewrite repository-relative references to the matching generated documentation-site locations."""
    for source, destination in {
        "../themes/README.md": "theme-format.md",
        "../themes/syntax/README.md": "synthwave84-source.md",
        "syntax/README.md": "synthwave84-source.md",
        "../../packaging/licenses/synthwave84-LICENSE-MIT": "licenses/synthwave84-LICENSE-MIT.txt",
        "../docs/syntax-themes.md": "syntax-themes.md",
        "../docs/themes.md": "themes.md",
        "../docs/development.md": "development.md",
        "../src/theme/interface.rs": "https://github.com/pbenner/rode/blob/main/src/theme/interface.rs",
        "../src/theme/definition.rs": "https://github.com/pbenner/rode/blob/main/src/theme/definition.rs",
        "../crates/rode-host-core/src/sandbox_policy.json": "https://github.com/pbenner/rode/blob/main/crates/rode-host-core/src/sandbox_policy.json",
    }.items():
        markdown = markdown.replace(f"]({source})", f"]({destination})")
    return markdown.replace("](../assets/", "](images/").replace("](../examples/", "](examples/")


def on_files(files, config):
    """Export canonical theme definitions, image assets and example notebooks into the MkDocs file set."""
    files.append(
        File.generated(config, "theme-format.md", content=(ROOT / "themes/README.md").read_text())
    )
    files.append(
        File.generated(
            config,
            "synthwave84-source.md",
            content=(ROOT / "themes/syntax/README.md").read_text(),
        )
    )
    files.append(
        File.generated(
            config,
            "licenses/synthwave84-LICENSE-MIT.txt",
            abs_src_path=str(ROOT / "packaging/licenses/synthwave84-LICENSE-MIT"),
        )
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
