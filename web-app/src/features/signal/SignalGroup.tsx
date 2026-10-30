import { useState } from 'react'
import { t } from '../../i18n'
import { Tabs } from '../../ui/Tabs'
import Overview from './Overview'
import { PageHeader } from '../../ui/primitives'
import Locking from './Locking'
import ImeiPage from './ImeiPage'

type Tab = 'overview' | 'locking' | 'imei'

export default function SignalGroup() {
  const [tab, setTab] = useState<Tab>('overview')

  return (
    <div className="space-y-4">
      <PageHeader title={t('Signal')} sub={t('Live radio metrics, band and cell locking')} />

      <Tabs
        tabs={[
          { id: 'overview', label: t('Overview') },
          { id: 'locking', label: t('Mode & Locking') },
          { id: 'imei', label: t('IMEI') },
        ]}
        active={tab}
        onChange={setTab}
      />

      {tab === 'overview' ? <Overview /> : tab === 'locking' ? <Locking /> : <ImeiPage />}
    </div>
  )
}
