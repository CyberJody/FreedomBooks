"""Dump the frozen app's named URL routes (name -> route with <converter:arg> parts) as JSON.

    FBV2_DB=fbv2_int_d1_screens . crates/fb-harness/py/env.sh
    $FB_PY crates/fb-screendiff/py/dump_urls.py > crates/fb-templates/urls.json
"""
import json
import os
import sys

sys.path.insert(0, os.environ["FB_REFERENCE"])
os.environ.setdefault("DJANGO_SETTINGS_MODULE", "config.settings")
import django  # noqa: E402

django.setup()
from django.urls import get_resolver  # noqa: E402
from django.urls.resolvers import URLPattern, URLResolver  # noqa: E402


def walk(patterns, prefix="", ns=""):
    for p in patterns:
        if isinstance(p, URLResolver):
            sub = ns
            if p.namespace:
                sub = f"{ns}{p.namespace}:" if not ns else f"{ns}{p.namespace}:"
            yield from walk(p.url_patterns, prefix + str(p.pattern), sub)
        elif isinstance(p, URLPattern) and p.name:
            yield f"{ns}{p.name}", "/" + prefix + str(p.pattern)


routes = {}
for name, route in walk(get_resolver().url_patterns):
    routes.setdefault(name, route)
print(json.dumps(routes, indent=1, sort_keys=True))
