-- Optional current business details; existing books receive no rows or changed values.
CREATE TABLE company_document_details (
    company_id TEXT NOT NULL PRIMARY KEY REFERENCES ledger_company(id),
    data TEXT NOT NULL CHECK(json_valid(data) AND json_type(data)='object' AND json_type(data,'$.street1') IS 'text' AND length(json_extract(data,'$.street1'))<=200 AND json_type(data,'$.street2') IS 'text' AND length(json_extract(data,'$.street2'))<=200 AND json_type(data,'$.city') IS 'text' AND length(json_extract(data,'$.city'))<=100 AND json_type(data,'$.region') IS 'text' AND length(json_extract(data,'$.region'))<=100 AND json_type(data,'$.postal_code') IS 'text' AND length(json_extract(data,'$.postal_code'))<=32 AND json_type(data,'$.country') IS 'text' AND length(json_extract(data,'$.country'))<=100 AND json_type(data,'$.phone') IS 'text' AND length(json_extract(data,'$.phone'))<=50 AND json_type(data,'$.email') IS 'text' AND length(json_extract(data,'$.email'))<=254 AND json_type(data,'$.website') IS 'text' AND length(json_extract(data,'$.website'))<=500 AND json_type(data,'$.tax_id') IS 'text' AND length(json_extract(data,'$.tax_id'))<=100 AND json_type(data,'$.payment_instructions') IS 'text' AND length(json_extract(data,'$.payment_instructions'))<=2000),
    logo_mime TEXT,
    logo BLOB,
    CHECK ((logo IS NULL AND logo_mime IS NULL) OR
           (typeof(logo)='blob' AND length(logo) BETWEEN 1 AND 131072 AND logo_mime IS NOT NULL AND logo_mime IN ('image/png','image/jpeg')))
);
