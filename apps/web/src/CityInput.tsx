import { useEffect, useState } from "react";

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

const places: Place[] = [
  { city: "Atlanta", region: "Georgia", country: "United States" },
  { city: "Austin", region: "Texas", country: "United States" },
  { city: "Baltimore", region: "Maryland", country: "United States" },
  { city: "Boston", region: "Massachusetts", country: "United States" },
  { city: "Charlotte", region: "North Carolina", country: "United States" },
  { city: "Chicago", region: "Illinois", country: "United States" },
  { city: "Dallas", region: "Texas", country: "United States" },
  { city: "Denver", region: "Colorado", country: "United States" },
  { city: "Detroit", region: "Michigan", country: "United States" },
  { city: "Houston", region: "Texas", country: "United States" },
  { city: "Las Vegas", region: "Nevada", country: "United States" },
  { city: "Los Angeles", region: "California", country: "United States" },
  { city: "Miami", region: "Florida", country: "United States" },
  { city: "Minneapolis", region: "Minnesota", country: "United States" },
  { city: "Nashville", region: "Tennessee", country: "United States" },
  { city: "New Orleans", region: "Louisiana", country: "United States" },
  { city: "New York City", region: "New York", country: "United States" },
  { city: "Orlando", region: "Florida", country: "United States" },
  { city: "Philadelphia", region: "Pennsylvania", country: "United States" },
  { city: "Phoenix", region: "Arizona", country: "United States" },
  { city: "Portland", region: "Oregon", country: "United States" },
  { city: "San Antonio", region: "Texas", country: "United States" },
  { city: "San Diego", region: "California", country: "United States" },
  { city: "San Francisco", region: "California", country: "United States" },
  { city: "Seattle", region: "Washington", country: "United States" },
  { city: "St. Louis", region: "Missouri", country: "United States" },
  { city: "Tampa", region: "Florida", country: "United States" },
  { city: "Washington", region: "District of Columbia", country: "United States" },
  { city: "Amsterdam", region: "North Holland", country: "Netherlands" },
  { city: "Barcelona", region: "Catalonia", country: "Spain" },
  { city: "Berlin", region: "Berlin", country: "Germany" },
  { city: "Dubai", region: "Dubai", country: "United Arab Emirates" },
  { city: "Dublin", region: "Leinster", country: "Ireland" },
  { city: "Hong Kong", region: "Hong Kong", country: "Hong Kong" },
  { city: "Lagos", region: "Lagos", country: "Nigeria" },
  { city: "London", region: "England", country: "United Kingdom" },
  { city: "Madrid", region: "Community of Madrid", country: "Spain" },
  { city: "Mexico City", region: "Mexico City", country: "Mexico" },
  { city: "Montreal", region: "Quebec", country: "Canada" },
  { city: "Paris", region: "Île-de-France", country: "France" },
  { city: "Rome", region: "Lazio", country: "Italy" },
  { city: "Singapore", region: "Singapore", country: "Singapore" },
  { city: "Sydney", region: "New South Wales", country: "Australia" },
  { city: "Tokyo", region: "Tokyo", country: "Japan" },
  { city: "Toronto", region: "Ontario", country: "Canada" },
  { city: "Vancouver", region: "British Columbia", country: "Canada" },
];

const unique = (values: string[]) => [...new Set(values)].sort((left, right) => left.localeCompare(right));
const countries = ["United States", ...unique(places.map(place => place.country).filter(country => country !== "United States"))];

function regionsFor(country: string) {
  return country === "United States"
    ? usRegions
    : unique(places.filter(place => place.country === country).map(place => place.region));
}

function citiesFor(country: string, region: string) {
  return unique(places.filter(place => place.country === country && place.region === region).map(place => place.city));
}

export function LocationPicker({
  id,
  city,
  region,
  country,
  onChange,
  className,
}: {
  id: string;
  city: string;
  region: string;
  country: string;
  onChange: (location: Location) => void;
  className?: string;
}) {
  const [countryChoice, setCountryChoice] = useState(() => country ? countries.includes(country) ? country : "other" : "");
  const [regionChoice, setRegionChoice] = useState(() => region ? regionsFor(country).includes(region) ? region : "other" : "");
  const [cityChoice, setCityChoice] = useState(() => city ? citiesFor(country, region).includes(city) ? city : "other" : "");

  useEffect(() => {
    if (country) setCountryChoice(countries.includes(country) ? country : "other");
    if (region) setRegionChoice(regionsFor(country).includes(region) ? region : "other");
    if (city) setCityChoice(citiesFor(country, region).includes(city) ? city : "other");
  }, [city, region, country]);

  const regionLabel = countryChoice === "United States" ? "State" : countryChoice === "Canada" ? "Province" : "State / region";
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
          {regionsFor(countryChoice).map(option => <option key={option} value={option}>{option}</option>)}
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
          {citiesFor(countryChoice, regionChoice).map(option => <option key={option} value={option}>{option}</option>)}
          {knownRegion && <option value="other">Other</option>}
        </select>
        {cityChoice === "other" && <input aria-label="City" value={city} onChange={event => onChange({ country, region, city: event.target.value })} maxLength={100} autoComplete="address-level2" placeholder="City" required />}
      </> : countryChoice === "other" ? <input id={`${id}-city`} value={city} onChange={event => onChange({ country, region, city: event.target.value })} maxLength={100} autoComplete="address-level2" placeholder="City" required /> : <select id={`${id}-city`} value="" disabled><option value="">Country first</option></select>}
    </div>
  </fieldset>;
}
