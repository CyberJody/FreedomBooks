"""Generated journal IDs use the smallest known scenario alias at each checkpoint.

Only ID-bearing paths call this helper: literal user text is never rewritten.
"""
def journal_names(r):
    names = {}
    for alias, key in r.refs['journal'].items():
        if key not in names or alias < names[key]:
            names[key] = alias
    return names
