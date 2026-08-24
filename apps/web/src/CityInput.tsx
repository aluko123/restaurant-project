import { useEffect, useState } from "react";
import type { ApiRequest } from "./SalesWorkspace";

type Location = { city: string; region: string; country: string };
type Place = Location;

const usRegions = [
  "Alabama", "Alaska", "Arizona", "Arkansas", "California", "Colorado", "Connecticut", "Delaware",
  "District of Columbia", "Florida", "Georgia", "Hawaii", "Idaho", "Illinois", "Indiana", "Iowa",
  "Kansas", "Kentucky", "Louisiana", "Maine", "Maryland", "Massachusetts", "Michigan", "Minnesota",
  "Mississippi", "Missouri", "Montana", "Nebraska", "Nevada", "New Hampshire", "New Jersey",
  "New Mexico", "New York", "North Carolina", "North Dakota", "Ohio", "Oklahoma", "Oregon",
  "Pennsylvania", "Rhode Island", "South Carolina", "South Dakota", "Tennessee", "Texas", "Utah",
  "Vermont", "Virginia", "Washington", "West Virginia", "Wisconsin", "Wyoming",
];

// Server-owned catalog of known cities (seeded, plus every city any
// restaurant added through an "Other" flow). Cached for the session.
let placeCache: Place[] | null = null;
let placeFetch: Promise<Place[]> | null = null;

function loadPlaces(request: ApiRequest): Promise<Place[]> {
  if (placeCache) return Promise.resolve(placeCache);
  placeFetch ??= request<Place[]>("/v1/location-options")
    .then((rows) => {
      placeCache = rows;
      return rows;
    })
    .catch(() => {
      // Unreachable server: the Other flows still work; retry next mount.
      placeFetch = null;
      return [];
    });
  return placeFetch;
}

const uniqueSorted = (values: string[]) =>
  [...new Set(values)].sort((left, right) => left.localeCompare(right));

function buildCountries(places: Place[]) {
  return [
    "United States",
    ...uniqueSorted(
      places.map(place => place.country).filter(country => country !== "United States"),
    ),
  ];
}

function regionsFor(places: Place[], country: string) {
  return country === "United States"
    ? usRegions
    : uniqueSorted(places.filter(place => place.country === country).map(place => place.region));
}

function citiesFor(places: Place[], country: string, region: string) {
  return uniqueSorted(
    places
      .filter(place => place.country === country && place.region === region)
      .map(place => place.city),
  );
}

export function LocationPicker({
  id,
  city,
  region,
  country,
  onChange,
  className,
  request,
}: {
  id: string;
  city: string;
  region: string;
  country: string;
  onChange: (location: Location) => void;
  className?: string;
  request: ApiRequest;
}) {
  const [places, setPlaces] = useState<Place[]>(() => placeCache ?? []);
  const countries = buildCountries(places);

  useEffect(() => {
    let cancelled = false;
    void loadPlaces(request).then(rows => {
      if (!cancelled) setPlaces(rows);
    });
    return () => {
      cancelled = true;
    };
  }, [request]);

  // Before the catalog arrives, dropdown choices start blank; the effect
  // below re-derives them once it loads.
  const [countryChoice, setCountryChoice] = useState(() =>
    placeCache ? (buildCountries(placeCache).includes(country) ? country : country ? "other" : "") : "",
  );
  const [regionChoice, setRegionChoice] = useState(() =>
    placeCache ? (regionsFor(placeCache, country).includes(region) ? region : region ? "other" : "") : "",
  );
  const [cityChoice, setCityChoice] = useState(() =>
    placeCache
      ? citiesFor(placeCache, country, region).includes(city)
        ? city
        : city
          ? "other"
          : ""
      : "",
  );

  // Once the catalog arrives (or props change programmatically), re-derive
  // which choices are known so dropdowns reflect them.
  useEffect(() => {
    if (country) setCountryChoice(countries.includes(country) ? country : "other");
    if (region) setRegionChoice(regionsFor(places, country).includes(region) ? region : "other");
    if (city) setCityChoice(citiesFor(places, country, region).includes(city) ? city : "other");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [city, region, country, places]);

  const regionLabel =
    countryChoice === "United States" ? "State" : countryChoice === "Canada" ? "Province" : "State / region";
  const knownCountry = countryChoice !== "" && countryChoice !== "other";
  const knownRegion = regionChoice !== "" && regionChoice !== "other";

  return <fieldset className={`location-picker${className ? ` ${className}` : ""}`}>
    <legend>Location</legend>

    <div className="location-part">
      <label htmlFor={`${id}-country`}>Country</label>
      <select id={`${id}-country`} value={countryChoice} required onChange={event => {
        const next = event.target.value;
        setCountryChoice(next);
        setRegionChoice("");
        setCityChoice("");
        onChange({ country: next === "other" ? "" : next, region: "", city: "" });
      }}>
        <option value="" disabled>Country</option>
        {countries.map(option => <option key={option} value={option}>{option}</option>)}
        <option value="other">Other</option>
      </select>
      {countryChoice === "other" && <input aria-label="Country" value={country} onChange={event => onChange({ country: event.target.value, region, city })} maxLength={100} autoComplete="country-name" placeholder="Country" required />}
    </div>

    <div className="location-part">
      <label htmlFor={`${id}-region`}>{regionLabel}</label>
      {knownCountry ? <>
        <select id={`${id}-region`} value={regionChoice} required onChange={event => {
          const next = event.target.value;
          setRegionChoice(next);
          setCityChoice("");
          onChange({ country, region: next === "other" ? "" : next, city: "" });
        }}>
          <option value="" disabled>{regionLabel}</option>
          {regionsFor(places, countryChoice).map(option => <option key={option} value={option}>{option}</option>)}
          <option value="other">Other {regionLabel.toLowerCase()}</option>
        </select>
        {regionChoice === "other" && <input aria-label={regionLabel} value={region} onChange={event => onChange({ country, region: event.target.value, city })} maxLength={100} autoComplete="address-level1" placeholder={regionLabel} required />}
      </> : countryChoice === "other" ? <input id={`${id}-region`} value={region} onChange={event => onChange({ country, region: event.target.value, city })} maxLength={100} autoComplete="address-level1" placeholder={regionLabel} required /> : <select id={`${id}-region`} value="" disabled><option value="">Country first</option></select>}
    </div>

    <div className="location-part">
      <label htmlFor={`${id}-city`}>City</label>
      {knownCountry ? <>
        <select id={`${id}-city`} value={cityChoice} required disabled={!knownRegion} onChange={event => {
          const next = event.target.value;
          setCityChoice(next);
          onChange({ country, region, city: next === "other" ? "" : next });
        }}>
          <option value="" disabled>{knownRegion ? "City" : `${regionLabel} first`}</option>
          {citiesFor(places, countryChoice, regionChoice).map(option => <option key={option} value={option}>{option}</option>)}
          {knownRegion && <option value="other">Other</option>}
        </select>
        {cityChoice === "other" && <input aria-label="City" value={city} onChange={event => onChange({ country, region, city: event.target.value })} maxLength={100} autoComplete="address-level2" placeholder="City" required />}
      </> : countryChoice === "other" ? <input id={`${id}-city`} value={city} onChange={event => onChange({ country, region, city: event.target.value })} maxLength={100} autoComplete="address-level2" placeholder="City" required /> : <select id={`${id}-city`} value="" disabled><option value="">Country first</option></select>}
    </div>
  </fieldset>;
}
