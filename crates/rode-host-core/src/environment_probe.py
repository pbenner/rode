"""Shared, bounded interpreter/kernelspec discovery source for local and SSH hosts."""


def project_environment_probe():
    """Return a standalone probe; inspecting environments never starts a kernel."""
    return r"""
import json
import sys

data = dict(executable=sys.executable, version=sys.version.split()[0],
            prefix=sys.prefix, base_prefix=sys.base_prefix, kernels=[], kernel_error="")
try:
    from jupyter_client.kernelspec import KernelSpecManager
    manager = KernelSpecManager()
    for name in sorted(manager.find_kernel_specs())[:64]:
        if not name or len(name) > 128 or not all(c.isascii() and (c.isalnum() or c in "._-") for c in name):
            continue
        try:
            spec = manager.get_kernel_spec(name)
            if spec.metadata.get("kernel_provisioner", {}).get("provisioner_name", "local-provisioner") == "local-provisioner":
                data["kernels"].append(dict(name=name, display_name=spec.display_name[:160], language=spec.language[:64]))
        except Exception:
            continue
except ImportError:
    data["kernel_error"] = "Install jupyter_client and ipykernel in the selected environment to run Python cells."
except Exception as error:
    data["kernel_error"] = str(error)[:512]
print(json.dumps(data))
"""
