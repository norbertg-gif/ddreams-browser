import { useRef, useState, type FormEvent, type KeyboardEvent,useEffect} from "react";

type Props={title:string;label:string;initialName:string;extension?:string;onSubmit:(name:string)=>Promise<void>;onClose:()=>void};
export default function FileNameDialog({title,label,initialName,extension,onSubmit,onClose}:Props){
  const [name,setName]=useState(initialName);const [error,setError]=useState("");const [saving,setSaving]=useState(false);const input=useRef<HTMLInputElement>(null);
  useEffect(()=>{input.current?.select()},[]);
  const submit=async(event:FormEvent)=>{event.preventDefault();if(saving)return;setSaving(true);setError("");try{await onSubmit(name)}catch(reason){setError(String(reason));input.current?.focus()}finally{setSaving(false)}};
  const keyDown=(event:KeyboardEvent)=>{if(event.key==="Escape"){event.preventDefault();onClose()}};
  return <div className="modal-backdrop"><form className="file-op-dialog file-name-dialog" onSubmit={e=>void submit(e)} onKeyDown={keyDown}>
    <header>{title}</header><label className="file-name-field"><span>{label}</span><div><input ref={input} autoFocus value={name} onChange={e=>{setName(e.target.value);setError("")}} />{extension&&<b>.{extension}</b>}</div></label>
    {error&&<div className="file-name-error" role="alert">{error}</div>}
    <footer><span/><button type="button" onClick={onClose}>Zrušiť</button><button type="submit" disabled={saving}>{saving?"Ukladám…":"OK"}</button></footer>
  </form></div>;
}
