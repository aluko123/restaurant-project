CREATE TABLE supplier_line_ignores (
    id UUID PRIMARY KEY,
    restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    supplier_key TEXT NOT NULL CHECK (BTRIM(supplier_key) <> ''),
    comparison_key TEXT NOT NULL CHECK (BTRIM(comparison_key) <> ''),
    comparison_unit TEXT NOT NULL DEFAULT '' CHECK (comparison_unit = BTRIM(comparison_unit)),
    created_by UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    FOREIGN KEY (restaurant_id, created_by)
        REFERENCES restaurant_memberships(restaurant_id, user_id)
        ON DELETE SET NULL (created_by),
    UNIQUE (restaurant_id, supplier_key, comparison_key, comparison_unit)
);

ALTER TABLE order_guide_lines
    DROP CONSTRAINT order_guide_lines_restaurant_id_supplier_mapping_id_fkey,
    ADD CONSTRAINT order_guide_lines_restaurant_id_supplier_mapping_id_fkey
        FOREIGN KEY (restaurant_id, supplier_mapping_id)
        REFERENCES supplier_product_mappings(restaurant_id, id)
        ON DELETE SET NULL (supplier_mapping_id);

-- The FK action must be able to detach even ordered/terminal guides, while every
-- snapshotted field remains immutable.
CREATE OR REPLACE FUNCTION guard_order_guide_line() RETURNS TRIGGER AS $$
DECLARE state TEXT;
BEGIN
  IF TG_OP='DELETE' THEN RAISE EXCEPTION 'order guide lines cannot be deleted'; END IF;
  IF TG_OP='UPDATE' AND OLD.supplier_mapping_id IS NOT NULL AND NEW.supplier_mapping_id IS NULL
     AND NOT EXISTS (
       SELECT 1 FROM supplier_product_mappings
       WHERE restaurant_id=OLD.restaurant_id AND id=OLD.supplier_mapping_id
     ) THEN
    NEW.supplier_mapping_id := OLD.supplier_mapping_id;
    IF NEW IS DISTINCT FROM OLD THEN RAISE EXCEPTION 'mapping deletion may not change guide evidence'; END IF;
    NEW.supplier_mapping_id := NULL;
    RETURN NEW;
  END IF;
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
