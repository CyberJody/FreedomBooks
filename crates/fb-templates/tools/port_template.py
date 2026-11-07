"""Mechanical first pass of a Django template -> minijinja port (fb-templates).

    python3 crates/fb-templates/tools/port_template.py <reference template> <output>

Rewrites only syntax with a fixed, faithful meaning; anything it does not know is left as is so the
minijinja compile (and the screen diff) fails loudly. What it rewrites:

  {% load ... %}                      removed (filters/functions are global in fb-templates)
  {% static 'p' %}                    {{ static('p') }}
  {% url 'name' a b %}                {{ url('name', a, b) }}
  {% icon a b c d %}                  {{ icon(a, b, c, d) }}          ({% icon_url x %} likewise)
  {% csrf_token %}                    {{ csrf_token }}                (the screen's command token input)
  {% empty %}                         {% else %}
  {% include 'x' with a=b c=d %}      {% with a=b, c=d %}{% include 'x' %}{% endwith %}
  forloop.counter / .last / .first    loop.index / loop.last / loop.first
  x.0.y                               x[0].y
  |f:arg                              |f(arg)
  != None / == None                   is not none / is none
  True / False / None (in tags)       true / false / none

Form widgets are NOT rewritten: `{{ field }}`, `{{ field.label_tag }}`, `{{ form.as_p }}` and error lists
need `|safe` on the pre-rendered strings of fb_templates::forms; do those by hand and keep a note.
"""
import re
import sys

TAG = re.compile(r"{%\s*(.*?)\s*%}", re.S)
VAR = re.compile(r"{{\s*(.*?)\s*}}", re.S)


def split_args(text):
    return re.findall(r"""'[^']*'|"[^"]*"|\S+""", text)


def expr(text):
    text = re.sub(r"\bforloop\.counter\b", "loop.index", text)
    text = re.sub(r"\bforloop\.last\b", "loop.last", text)
    text = re.sub(r"\bforloop\.first\b", "loop.first", text)
    text = re.sub(r"\.(\d+)\b", r"[\1]", text)
    text = re.sub(r"\|(\w+):('[^']*'|\"[^\"]*\"|[\w.\[\]]+)", r"|\1(\2)", text)
    text = re.sub(r"!=\s*None\b", "is not none", text)
    text = re.sub(r"==\s*None\b", "is none", text)
    text = re.sub(r"\bTrue\b", "true", text)
    text = re.sub(r"\bFalse\b", "false", text)
    text = re.sub(r"\bNone\b", "none", text)
    return text


def tag(match):
    body = match.group(1)
    word, _, rest = body.partition(" ")
    if word == "load":
        return ""
    if word == "static":
        return "{{ static(%s) }}" % expr(rest)
    if word in ("url", "icon", "icon_url"):
        args = [expr(a) for a in split_args(rest)]
        if "as" in args:
            raise SystemExit(f"unsupported '{{% {body} %}}' (as-assignment): port by hand")
        return "{{ %s(%s) }}" % (word, ", ".join(args))
    if word == "csrf_token":
        return "{{ csrf_token }}"
    if word == "empty":
        return "{% else %}"
    if word == "include" and " with " in rest:
        name, _, assigns = rest.partition(" with ")
        pairs = ", ".join(expr(a) for a in split_args(assigns))
        return "{%% with %s %%}{%% include %s %%}{%% endwith %%}" % (pairs, name.strip())
    if word in ("if", "elif", "for", "with", "include", "extends", "block"):
        return "{%% %s %s %%}" % (word, expr(rest)) if rest else "{%% %s %%}" % word
    return match.group(0)


def port(text):
    text = TAG.sub(tag, text)
    text = VAR.sub(lambda m: "{{ %s }}" % expr(m.group(1)), text)
    return text


if __name__ == "__main__":
    src, dst = sys.argv[1], sys.argv[2]
    with open(src, encoding="utf-8") as fh:
        ported = port(fh.read())
    with open(dst, "w", encoding="utf-8") as fh:
        fh.write(ported)
