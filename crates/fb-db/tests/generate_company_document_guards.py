"""Append company-document boundaries without rewriting any published migration.

The prior shared view arms and classifications remain intact. Running this twice is
idempotent; the independent Rust inventories verify the live schema and guards.
"""
import json
from collections import Counter
import generate_value_guards as base


def main():
    base.MIGRATION = '0181_company_document_guards'
    _, inventory = base.schema()
    path = base.HERE / 'fb_value_guard_inventory.json'
    prior = json.loads(path.read_text())
    prior.pop('company_document_details', None)
    assert set(inventory) - set(prior) == {'company_document_details'}
    for table, columns in prior.items():
        assert set(columns) == set(inventory[table])
        inventory[table] = columns
    assert inventory['company_document_details'] == {'company_id': 'uuid', 'data': 'text', 'logo_mime': 'text'}
    out = ['-- Company-document shared value/JSON guards and permanent company identity; no data changes.']
    for event in ('INSERT', 'UPDATE'):
        out.append(f"CREATE TRIGGER fb_valid_company_document_details_{event.lower()} BEFORE {event} ON company_document_details BEGIN\n"
                   f" SELECT RAISE(ABORT,'{base.MESSAGE}') WHERE\n {base.condition('NEW.', inventory['company_document_details'])};\nEND;")
    out.append('DROP VIEW fb_value_invalid;')
    value_sql = (base.FBDB / 'sql/0161_credential_validators.sql').read_text()
    value_view = value_sql.split('CREATE VIEW fb_value_invalid AS\n', 1)[1].split(';', 1)[0]
    out.append('CREATE VIEW fb_value_invalid AS\n' + value_view +
               "\nUNION ALL SELECT 'invalid stored value in company_document_details' AS problem FROM company_document_details WHERE\n " +
               base.condition('', inventory['company_document_details']) + ';')
    # Preserve every prior JSON arm verbatim, including assistant and estimates.
    sql = (base.FBDB / 'sql/0131_estimates.sql').read_text()
    view = sql.split('CREATE VIEW fb_json_invalid AS\n', 1)[1].split(';', 1)[0]
    out.append('DROP VIEW fb_json_invalid;')
    out.append("CREATE VIEW fb_json_invalid AS\n" + view + "\nUNION ALL SELECT 'company_document_details.data:'||company_id FROM company_document_details WHERE NOT fb_json_unique_keys(data);")
    for event in ('INSERT', 'UPDATE'):
        out.append(f"CREATE TRIGGER fb_json_company_document_details_{event.lower()} BEFORE {event} ON company_document_details BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;")
    out.extend([
        "CREATE TRIGGER company_documents_identity_update BEFORE UPDATE OF company_id ON company_document_details WHEN NEW.company_id IS NOT OLD.company_id BEGIN SELECT RAISE(ABORT,'company_documents_identity: Business details identity is permanent'); END;",
        "CREATE TRIGGER company_documents_identity_delete BEFORE DELETE ON company_document_details BEGIN SELECT RAISE(ABORT,'company_documents_identity: Business details identity is permanent'); END;",
        "CREATE TRIGGER company_documents_identity_insert BEFORE INSERT ON company_document_details WHEN EXISTS(SELECT 1 FROM company_document_details WHERE company_id=NEW.company_id) BEGIN SELECT RAISE(ABORT,'company_documents_identity: Business details identity is permanent'); END;",
    ])
    (base.FBDB / 'sql' / (base.MIGRATION + '.sql')).write_text('\n'.join(out) + '\n')
    path.write_text(json.dumps(inventory, indent=1, sort_keys=True) + '\n')
    print(len(inventory), sum(map(len, inventory.values())), dict(Counter(k for c in inventory.values() for k in c.values())))


if __name__ == '__main__':
    main()
