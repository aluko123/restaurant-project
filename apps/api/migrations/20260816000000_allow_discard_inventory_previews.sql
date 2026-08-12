-- Managers may discard previews that have not created inventory.
CREATE OR REPLACE FUNCTION guard_inventory_import() RETURNS TRIGGER AS $$ BEGIN
  IF TG_OP='DELETE' THEN
    IF OLD.status='needs_review' THEN RETURN OLD; END IF;
    RAISE EXCEPTION 'applied imports cannot be deleted';
  END IF;
  IF OLD.status='applied' THEN RAISE EXCEPTION 'applied imports are immutable'; END IF;
  IF NEW.id<>OLD.id OR NEW.restaurant_id<>OLD.restaurant_id OR NEW.content_hash<>OLD.content_hash OR NEW.created_by<>OLD.created_by OR NEW.created_at<>OLD.created_at OR NEW.revision<>OLD.revision+1 OR NEW.status<>'applied' THEN
    RAISE EXCEPTION 'invalid import transition'; END IF; RETURN NEW; END $$ LANGUAGE plpgsql;
