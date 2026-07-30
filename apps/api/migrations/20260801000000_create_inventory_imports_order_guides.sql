-- Inventory CSV imports and count-backed order guides.
ALTER TABLE inventory_count_sessions ADD CONSTRAINT inventory_count_sessions_restaurant_id_id_key UNIQUE (restaurant_id,id);
ALTER TABLE supplier_product_mappings ADD CONSTRAINT supplier_product_mappings_restaurant_id_id_key UNIQUE (restaurant_id,id);

CREATE TABLE inventory_imports (
    id UUID PRIMARY KEY,
    restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    original_filename TEXT NOT NULL CHECK (BTRIM(original_filename) <> '' AND CHAR_LENGTH(original_filename) <= 255),
    content_hash CHAR(64) NOT NULL,
    status TEXT NOT NULL DEFAULT 'needs_review' CHECK (status IN ('needs_review','applied')),
    revision BIGINT NOT NULL DEFAULT 0 CHECK (revision >= 0),
    created_by UUID NOT NULL REFERENCES users(id), applied_by UUID REFERENCES users(id), applied_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(), updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (restaurant_id,content_hash), UNIQUE (restaurant_id,id),
    CHECK ((status='needs_review' AND applied_by IS NULL AND applied_at IS NULL) OR
           (status='applied' AND applied_by IS NOT NULL AND applied_at IS NOT NULL))
);
CREATE TABLE inventory_import_rows (
    id UUID PRIMARY KEY, restaurant_id UUID NOT NULL, import_id UUID NOT NULL,
    row_number INTEGER NOT NULL CHECK (row_number >= 2), name TEXT NOT NULL, category TEXT,
    count_unit TEXT NOT NULL, par_level TEXT, validation_errors JSONB NOT NULL DEFAULT '[]'::jsonb,
    selected BOOLEAN, created_inventory_item_id UUID,
    UNIQUE(import_id,row_number),
    FOREIGN KEY (restaurant_id,import_id) REFERENCES inventory_imports(restaurant_id,id) ON DELETE CASCADE,
    FOREIGN KEY (restaurant_id,created_inventory_item_id) REFERENCES inventory_items(restaurant_id,id),
    CHECK ((selected IS DISTINCT FROM TRUE AND created_inventory_item_id IS NULL) OR
           (selected IS TRUE AND created_inventory_item_id IS NOT NULL))
);

CREATE TABLE order_guides (
    id UUID PRIMARY KEY, restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    source_count_id UUID NOT NULL, status TEXT NOT NULL DEFAULT 'draft' CHECK(status IN ('draft','ordered','received','cancelled')),
    revision BIGINT NOT NULL DEFAULT 0 CHECK(revision >= 0), created_by UUID NOT NULL REFERENCES users(id),
    ordered_by UUID REFERENCES users(id), ordered_at TIMESTAMPTZ, received_by UUID REFERENCES users(id), received_at TIMESTAMPTZ,
    cancelled_by UUID REFERENCES users(id), cancelled_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(), updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(restaurant_id,source_count_id), UNIQUE(restaurant_id,id),
    FOREIGN KEY (restaurant_id,source_count_id) REFERENCES inventory_count_sessions(restaurant_id,id),
    CHECK ((status='draft' AND ordered_by IS NULL AND ordered_at IS NULL AND received_by IS NULL AND received_at IS NULL AND cancelled_by IS NULL AND cancelled_at IS NULL) OR
      (status='ordered' AND ordered_by IS NOT NULL AND ordered_at IS NOT NULL AND received_by IS NULL AND received_at IS NULL AND cancelled_by IS NULL AND cancelled_at IS NULL) OR
      (status='received' AND ordered_by IS NOT NULL AND ordered_at IS NOT NULL AND received_by IS NOT NULL AND received_at IS NOT NULL AND cancelled_by IS NULL AND cancelled_at IS NULL) OR
      (status='cancelled' AND cancelled_by IS NOT NULL AND cancelled_at IS NOT NULL AND received_by IS NULL AND received_at IS NULL))
);
CREATE UNIQUE INDEX order_guides_one_open_idx ON order_guides(restaurant_id) WHERE status IN ('draft','ordered');

CREATE TABLE order_guide_lines (
    id UUID PRIMARY KEY, restaurant_id UUID NOT NULL, guide_id UUID NOT NULL, inventory_item_id UUID NOT NULL,
    inventory_item_name TEXT NOT NULL, count_unit TEXT NOT NULL,
    counted_quantity NUMERIC(18,6) NOT NULL CHECK(counted_quantity >= 0 AND counted_quantity <> 'NaN'::numeric),
    par_level NUMERIC(18,6) NOT NULL CHECK(par_level >= 0 AND par_level <> 'NaN'::numeric),
    shortage NUMERIC(18,6) NOT NULL CHECK(shortage > 0 AND shortage <> 'NaN'::numeric),
    supplier_mapping_id UUID, supplier_name TEXT CHECK(supplier_name IS NULL OR (BTRIM(supplier_name) <> '' AND CHAR_LENGTH(supplier_name)<=120)),
    product_description TEXT, supplier_sku TEXT, order_unit TEXT NOT NULL,
    count_units_per_order_unit NUMERIC(30,12) NOT NULL CHECK(count_units_per_order_unit > 0 AND count_units_per_order_unit <> 'NaN'::numeric),
    suggested_order_quantity NUMERIC(30,12) NOT NULL CHECK(suggested_order_quantity > 0 AND suggested_order_quantity <> 'NaN'::numeric),
    order_quantity NUMERIC(30,12) NOT NULL CHECK(order_quantity > 0 AND order_quantity <> 'NaN'::numeric),
    received_quantity NUMERIC(30,12) CHECK(received_quantity >= 0 AND received_quantity <> 'NaN'::numeric),
    receipt_status TEXT CHECK(receipt_status IN ('received','missing')), received_by UUID REFERENCES users(id), received_at TIMESTAMPTZ,
    UNIQUE(guide_id,inventory_item_id),
    FOREIGN KEY (restaurant_id,guide_id) REFERENCES order_guides(restaurant_id,id) ON DELETE CASCADE,
    FOREIGN KEY (restaurant_id,inventory_item_id) REFERENCES inventory_items(restaurant_id,id),
    FOREIGN KEY (restaurant_id,supplier_mapping_id) REFERENCES supplier_product_mappings(restaurant_id,id),
    CHECK ((received_quantity IS NULL AND receipt_status IS NULL AND received_by IS NULL AND received_at IS NULL) OR
      (received_quantity = 0 AND receipt_status='missing' AND received_by IS NOT NULL AND received_at IS NOT NULL) OR
      (received_quantity > 0 AND receipt_status='received' AND received_by IS NOT NULL AND received_at IS NOT NULL))
);

CREATE FUNCTION guard_inventory_import() RETURNS TRIGGER AS $$ BEGIN
  IF TG_OP='DELETE' THEN RAISE EXCEPTION 'inventory imports cannot be deleted'; END IF;
  IF OLD.status='applied' THEN RAISE EXCEPTION 'applied imports are immutable'; END IF;
  IF NEW.id<>OLD.id OR NEW.restaurant_id<>OLD.restaurant_id OR NEW.content_hash<>OLD.content_hash OR NEW.created_by<>OLD.created_by OR NEW.created_at<>OLD.created_at OR NEW.revision<>OLD.revision+1 OR NEW.status<>'applied' THEN
    RAISE EXCEPTION 'invalid import transition'; END IF; RETURN NEW; END $$ LANGUAGE plpgsql;
CREATE TRIGGER inventory_import_guard BEFORE UPDATE OR DELETE ON inventory_imports FOR EACH ROW EXECUTE FUNCTION guard_inventory_import();
CREATE FUNCTION guard_inventory_import_row() RETURNS TRIGGER AS $$ DECLARE parent_status TEXT; BEGIN
  IF TG_OP='UPDATE' AND (NEW.id<>OLD.id OR NEW.restaurant_id<>OLD.restaurant_id OR NEW.import_id<>OLD.import_id OR NEW.row_number<>OLD.row_number) THEN RAISE EXCEPTION 'import row identity is immutable'; END IF;
  SELECT status INTO parent_status FROM inventory_imports WHERE id=CASE WHEN TG_OP='DELETE' THEN OLD.import_id ELSE NEW.import_id END AND restaurant_id=CASE WHEN TG_OP='DELETE' THEN OLD.restaurant_id ELSE NEW.restaurant_id END FOR UPDATE;
  IF parent_status='applied' THEN RAISE EXCEPTION 'applied import rows are immutable'; END IF;
  RETURN CASE WHEN TG_OP='DELETE' THEN OLD ELSE NEW END; END $$ LANGUAGE plpgsql;
CREATE TRIGGER inventory_import_row_guard BEFORE INSERT OR UPDATE OR DELETE ON inventory_import_rows FOR EACH ROW EXECUTE FUNCTION guard_inventory_import_row();

CREATE FUNCTION guard_order_guide() RETURNS TRIGGER AS $$ BEGIN
  IF TG_OP='DELETE' THEN RAISE EXCEPTION 'order guides cannot be deleted'; END IF;
  IF OLD.status IN ('received','cancelled') THEN RAISE EXCEPTION 'terminal order guides are immutable'; END IF;
  IF NEW.id<>OLD.id OR NEW.restaurant_id<>OLD.restaurant_id OR NEW.source_count_id<>OLD.source_count_id OR NEW.created_by<>OLD.created_by OR NEW.created_at<>OLD.created_at OR NEW.revision<>OLD.revision+1 THEN RAISE EXCEPTION 'invalid guide update'; END IF;
  IF NOT ((OLD.status='draft' AND NEW.status IN ('draft','ordered','cancelled')) OR (OLD.status='ordered' AND NEW.status IN ('ordered','received','cancelled'))) THEN RAISE EXCEPTION 'invalid guide transition'; END IF;
  IF OLD.status='ordered' AND (NEW.ordered_by IS DISTINCT FROM OLD.ordered_by OR NEW.ordered_at IS DISTINCT FROM OLD.ordered_at) THEN RAISE EXCEPTION 'ordered evidence is immutable'; END IF;
  RETURN NEW; END $$ LANGUAGE plpgsql;
CREATE TRIGGER order_guide_guard BEFORE UPDATE OR DELETE ON order_guides FOR EACH ROW EXECUTE FUNCTION guard_order_guide();
CREATE FUNCTION guard_order_guide_line() RETURNS TRIGGER AS $$ DECLARE state TEXT; BEGIN
  IF TG_OP='DELETE' THEN RAISE EXCEPTION 'order guide lines cannot be deleted'; END IF;
  SELECT status INTO state FROM order_guides WHERE id=NEW.guide_id AND restaurant_id=NEW.restaurant_id FOR UPDATE;
  IF state IS NULL THEN RAISE EXCEPTION 'order guide parent is missing'; END IF;
  IF TG_OP='INSERT' THEN
    IF state<>'draft' OR NEW.received_quantity IS NOT NULL OR NEW.receipt_status IS NOT NULL OR NEW.received_by IS NOT NULL OR NEW.received_at IS NOT NULL THEN RAISE EXCEPTION 'new guide lines require a draft guide'; END IF;
    RETURN NEW;
  END IF;
  IF NEW.id<>OLD.id OR NEW.restaurant_id<>OLD.restaurant_id OR NEW.guide_id<>OLD.guide_id OR NEW.inventory_item_id<>OLD.inventory_item_id OR NEW.inventory_item_name<>OLD.inventory_item_name OR NEW.count_unit<>OLD.count_unit OR NEW.counted_quantity<>OLD.counted_quantity OR NEW.par_level<>OLD.par_level OR NEW.shortage<>OLD.shortage THEN RAISE EXCEPTION 'order guide evidence is immutable'; END IF;
  IF state='draft' THEN
    IF NEW.received_quantity IS NOT NULL OR NEW.receipt_status IS NOT NULL OR NEW.received_by IS NOT NULL OR NEW.received_at IS NOT NULL THEN RAISE EXCEPTION 'draft guide lines cannot be received'; END IF;
  ELSIF state='ordered' THEN
    IF NEW.supplier_mapping_id IS DISTINCT FROM OLD.supplier_mapping_id OR NEW.supplier_name IS DISTINCT FROM OLD.supplier_name OR NEW.product_description IS DISTINCT FROM OLD.product_description OR NEW.supplier_sku IS DISTINCT FROM OLD.supplier_sku OR NEW.order_unit<>OLD.order_unit OR NEW.count_units_per_order_unit<>OLD.count_units_per_order_unit OR NEW.suggested_order_quantity<>OLD.suggested_order_quantity OR NEW.order_quantity<>OLD.order_quantity THEN RAISE EXCEPTION 'ordered guide evidence is immutable'; END IF;
    IF OLD.received_quantity IS NOT NULL OR OLD.receipt_status IS NOT NULL OR OLD.received_by IS NOT NULL OR OLD.received_at IS NOT NULL THEN RAISE EXCEPTION 'received line evidence is immutable'; END IF;
    IF NEW.received_quantity IS NULL OR NEW.receipt_status IS NULL OR NEW.received_by IS NULL OR NEW.received_at IS NULL THEN RAISE EXCEPTION 'ordered line receipt must be complete'; END IF;
  ELSE RAISE EXCEPTION 'terminal guide lines are immutable'; END IF;
  RETURN NEW; END $$ LANGUAGE plpgsql;
CREATE TRIGGER order_guide_line_guard BEFORE INSERT OR UPDATE OR DELETE ON order_guide_lines FOR EACH ROW EXECUTE FUNCTION guard_order_guide_line();
