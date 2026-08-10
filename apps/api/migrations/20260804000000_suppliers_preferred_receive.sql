-- Canonical suppliers, preferred supplier on items, receive discrepancy + invoice link.

CREATE TABLE suppliers (
    id UUID PRIMARY KEY,
    restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (BTRIM(name) <> '' AND CHAR_LENGTH(name) <= 120),
    name_key TEXT NOT NULL CHECK (BTRIM(name_key) <> ''),
    archived_at TIMESTAMPTZ,
    created_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (restaurant_id, id),
    UNIQUE (restaurant_id, name_key)
);

CREATE INDEX suppliers_restaurant_active_idx
    ON suppliers (restaurant_id, archived_at NULLS FIRST, LOWER(name), id);

-- Backfill suppliers from free-text history (mappings, invoices, receipts, guides).
WITH names AS (
    SELECT restaurant_id, BTRIM(supplier_name) AS name, LOWER(BTRIM(supplier_name)) AS name_key
    FROM supplier_product_mappings
    WHERE BTRIM(supplier_name) <> ''
    UNION ALL
    SELECT restaurant_id, BTRIM(supplier_name), LOWER(BTRIM(supplier_name))
    FROM invoices
    WHERE BTRIM(supplier_name) <> ''
    UNION ALL
    SELECT restaurant_id, BTRIM(supplier_name), LOWER(BTRIM(supplier_name))
    FROM purchase_receipts
    WHERE BTRIM(supplier_name) <> ''
    UNION ALL
    SELECT restaurant_id, BTRIM(supplier_name), LOWER(BTRIM(supplier_name))
    FROM order_guide_lines
    WHERE supplier_name IS NOT NULL AND BTRIM(supplier_name) <> ''
),
picked AS (
    SELECT DISTINCT ON (restaurant_id, name_key)
        restaurant_id, name, name_key
    FROM names
    ORDER BY restaurant_id, name_key, CHAR_LENGTH(name) DESC, name
),
owners AS (
    SELECT DISTINCT ON (restaurant_id) restaurant_id, user_id
    FROM restaurant_memberships
    WHERE role = 'owner'
    ORDER BY restaurant_id, user_id
)
INSERT INTO suppliers (id, restaurant_id, name, name_key, created_by)
SELECT gen_random_uuid(), p.restaurant_id, p.name, p.name_key, o.user_id
FROM picked p
JOIN owners o ON o.restaurant_id = p.restaurant_id;

ALTER TABLE supplier_product_mappings
    ADD COLUMN supplier_id UUID,
    ADD CONSTRAINT supplier_product_mappings_supplier_fk
        FOREIGN KEY (restaurant_id, supplier_id) REFERENCES suppliers(restaurant_id, id);

UPDATE supplier_product_mappings m
SET supplier_id = s.id
FROM suppliers s
WHERE s.restaurant_id = m.restaurant_id
  AND s.name_key = m.supplier_key;

ALTER TABLE inventory_items
    ADD COLUMN preferred_supplier_id UUID REFERENCES suppliers(id) ON DELETE SET NULL;

CREATE INDEX inventory_items_preferred_supplier_idx
    ON inventory_items (restaurant_id, preferred_supplier_id)
    WHERE preferred_supplier_id IS NOT NULL;

ALTER TABLE order_guides
    ADD COLUMN linked_invoice_id UUID,
    ADD CONSTRAINT order_guides_linked_invoice_fk
        FOREIGN KEY (restaurant_id, linked_invoice_id) REFERENCES invoices(restaurant_id, id);

ALTER TABLE order_guide_lines
    ADD COLUMN supplier_id UUID,
    ADD COLUMN discrepancy_kind TEXT
        CHECK (discrepancy_kind IS NULL OR discrepancy_kind IN ('none', 'short', 'over', 'missing')),
    ADD CONSTRAINT order_guide_lines_supplier_fk
        FOREIGN KEY (restaurant_id, supplier_id) REFERENCES suppliers(restaurant_id, id);

UPDATE order_guide_lines l
SET supplier_id = s.id
FROM suppliers s
WHERE s.restaurant_id = l.restaurant_id
  AND l.supplier_name IS NOT NULL
  AND s.name_key = LOWER(BTRIM(l.supplier_name));

ALTER TABLE order_guide_lines DROP CONSTRAINT order_guide_lines_check;

ALTER TABLE order_guide_lines
    ADD CONSTRAINT order_guide_lines_receipt_check CHECK (
        (received_quantity IS NULL AND receipt_status IS NULL AND discrepancy_kind IS NULL
            AND received_by IS NULL AND received_at IS NULL)
        OR (received_quantity = 0 AND receipt_status = 'missing' AND discrepancy_kind = 'missing'
            AND received_by IS NOT NULL AND received_at IS NOT NULL)
        OR (received_quantity > 0 AND receipt_status = 'received'
            AND discrepancy_kind IN ('none', 'short', 'over')
            AND received_by IS NOT NULL AND received_at IS NOT NULL)
    );

CREATE OR REPLACE FUNCTION guard_order_guide_line() RETURNS TRIGGER AS $$
DECLARE state TEXT;
BEGIN
  IF TG_OP='DELETE' THEN RAISE EXCEPTION 'order guide lines cannot be deleted'; END IF;
  SELECT status INTO state FROM order_guides WHERE id=NEW.guide_id AND restaurant_id=NEW.restaurant_id FOR UPDATE;
  IF state IS NULL THEN RAISE EXCEPTION 'order guide parent is missing'; END IF;
  IF TG_OP='INSERT' THEN
    IF state<>'draft' OR NEW.received_quantity IS NOT NULL OR NEW.receipt_status IS NOT NULL
       OR NEW.discrepancy_kind IS NOT NULL OR NEW.received_by IS NOT NULL OR NEW.received_at IS NOT NULL THEN
      RAISE EXCEPTION 'new guide lines require a draft guide';
    END IF;
    RETURN NEW;
  END IF;
  IF NEW.id<>OLD.id OR NEW.restaurant_id<>OLD.restaurant_id OR NEW.guide_id<>OLD.guide_id
     OR NEW.inventory_item_id<>OLD.inventory_item_id OR NEW.inventory_item_name<>OLD.inventory_item_name
     OR NEW.count_unit<>OLD.count_unit OR NEW.counted_quantity<>OLD.counted_quantity
     OR NEW.par_level<>OLD.par_level OR NEW.shortage<>OLD.shortage THEN
    RAISE EXCEPTION 'order guide evidence is immutable';
  END IF;
  IF state='draft' THEN
    IF NEW.received_quantity IS NOT NULL OR NEW.receipt_status IS NOT NULL
       OR NEW.discrepancy_kind IS NOT NULL OR NEW.received_by IS NOT NULL OR NEW.received_at IS NOT NULL THEN
      RAISE EXCEPTION 'draft guide lines cannot be received';
    END IF;
  ELSIF state='ordered' THEN
    IF NEW.supplier_mapping_id IS DISTINCT FROM OLD.supplier_mapping_id
       OR NEW.supplier_id IS DISTINCT FROM OLD.supplier_id
       OR NEW.supplier_name IS DISTINCT FROM OLD.supplier_name
       OR NEW.product_description IS DISTINCT FROM OLD.product_description
       OR NEW.supplier_sku IS DISTINCT FROM OLD.supplier_sku
       OR NEW.order_unit<>OLD.order_unit
       OR NEW.count_units_per_order_unit<>OLD.count_units_per_order_unit
       OR NEW.suggested_order_quantity<>OLD.suggested_order_quantity
       OR NEW.order_quantity<>OLD.order_quantity THEN
      RAISE EXCEPTION 'ordered guide evidence is immutable';
    END IF;
    IF OLD.received_quantity IS NOT NULL OR OLD.receipt_status IS NOT NULL
       OR OLD.discrepancy_kind IS NOT NULL OR OLD.received_by IS NOT NULL OR OLD.received_at IS NOT NULL THEN
      RAISE EXCEPTION 'received line evidence is immutable';
    END IF;
    IF NEW.received_quantity IS NULL OR NEW.receipt_status IS NULL OR NEW.discrepancy_kind IS NULL
       OR NEW.received_by IS NULL OR NEW.received_at IS NULL THEN
      RAISE EXCEPTION 'ordered line receipt must be complete';
    END IF;
  ELSE
    RAISE EXCEPTION 'terminal guide lines are immutable';
  END IF;
  RETURN NEW;
END $$ LANGUAGE plpgsql;
